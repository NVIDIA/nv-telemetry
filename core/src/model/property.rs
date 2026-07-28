// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::error::Error;
use std::fmt;
use std::sync::Arc;

use super::collection::sort_and_find_duplicate;
use super::collection::sorted_collection;
use super::DurationValue;
use super::Finite;
use super::Name;
use super::NonFiniteError;
use super::Subject;
use super::Timestamp;

/// A source reference that may or may not resolve to a canonical subject.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ResourceReference {
    pub source_key: Name,
    pub subject: Option<Subject>,
}

impl ResourceReference {
    pub fn new(source_key: impl Into<Name>) -> Self {
        Self {
            source_key: source_key.into(),
            subject: None,
        }
    }

    #[must_use]
    pub fn with_subject(mut self, subject: Subject) -> Self {
        self.subject = Some(subject);
        self
    }
}

/// A recursively structured, source-neutral resource property value.
///
/// [`PropertyValue::Null`] means the source reported an explicit null, which
/// is distinct from the property being absent from a map. Absence in a
/// [`ResourceCompleteness::Complete`] resource means the device does not
/// implement the property; absence in a partial resource means nothing.
///
/// The encoding is adjacently tagged so every variant has the same shape, so
/// decoding needs a self-describing format. JSON, CBOR and `MessagePack`
/// qualify; bincode can write this type but not read it back.
///
/// [`ResourceCompleteness::Complete`]: super::ResourceCompleteness::Complete
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "serde",
    serde(tag = "type", content = "value", rename_all = "snake_case")
)]
#[non_exhaustive]
pub enum PropertyValue {
    Null,
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(Finite),
    String(Arc<str>),
    #[cfg_attr(feature = "serde", serde(with = "hex_bytes"))]
    Bytes(Arc<[u8]>),
    Timestamp(Timestamp),
    Duration(DurationValue),
    Reference(ResourceReference),
    Array(Box<[PropertyValue]>),
    Object(PropertyMap),
}

impl PropertyValue {
    /// Builds a floating point property, rejecting non-finite input.
    ///
    /// # Errors
    ///
    /// Returns [`NonFiniteError`] if the value is `NaN` or an infinity.
    pub const fn f64(value: f64) -> Result<Self, NonFiniteError> {
        match Finite::new(value) {
            Ok(value) => Ok(Self::F64(value)),
            Err(error) => Err(error),
        }
    }
}

impl From<bool> for PropertyValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for PropertyValue {
    fn from(value: i64) -> Self {
        Self::I64(value)
    }
}

impl From<u64> for PropertyValue {
    fn from(value: u64) -> Self {
        Self::U64(value)
    }
}

impl From<Finite> for PropertyValue {
    fn from(value: Finite) -> Self {
        Self::F64(value)
    }
}

impl TryFrom<f64> for PropertyValue {
    type Error = NonFiniteError;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::f64(value)
    }
}

impl From<String> for PropertyValue {
    fn from(value: String) -> Self {
        Self::String(value.into())
    }
}

impl From<&str> for PropertyValue {
    fn from(value: &str) -> Self {
        Self::String(value.into())
    }
}

impl From<Timestamp> for PropertyValue {
    fn from(value: Timestamp) -> Self {
        Self::Timestamp(value)
    }
}

impl From<DurationValue> for PropertyValue {
    fn from(value: DurationValue) -> Self {
        Self::Duration(value)
    }
}

impl From<ResourceReference> for PropertyValue {
    fn from(value: ResourceReference) -> Self {
        Self::Reference(value)
    }
}

impl From<PropertyMap> for PropertyValue {
    fn from(value: PropertyMap) -> Self {
        Self::Object(value)
    }
}

/// One named resource property.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Property {
    pub name: Name,
    pub value: PropertyValue,
}

impl Property {
    pub fn new(name: impl Into<Name>, value: impl Into<PropertyValue>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// An immutable, key-sorted object property map.
///
/// Sorting by name makes the encoding canonical, so two maps carrying the same
/// properties hash identically regardless of the order the source reported
/// them.
#[derive(Clone)]
pub struct PropertyMap(Arc<PropertyEntries>);

impl PropertyMap {
    /// Maximum number of nesting levels allowed below a property map.
    ///
    /// Property values are recursive, so an unbounded structure would overflow
    /// the stack when walked. Every value reachable from a stored map is
    /// within this bound, which makes recursive walks and the recursive drop
    /// of a stored map safe by construction.
    ///
    /// The bound is set by what a decoder will read back, not by what a walk
    /// can survive. [`PropertyValue`] is adjacently tagged, so one level here
    /// costs two or three levels of a decoder's own recursion limit, and the
    /// batch wrapping the map costs more again; `serde_json` stops at 128 and
    /// so rejects a bare map nested past 41. Accepting more would let this
    /// crate build and encode a graph that a consumer of the same crate
    /// cannot decode, which is a delivery failure no error surfaces. The
    /// margin below the observed ceiling absorbs future envelope levels.
    pub const MAX_DEPTH: u32 = 32;

    /// Sorts properties by name and rejects duplicates and excessive nesting.
    ///
    /// Rejected input is dismantled without recursion, so handing this an
    /// arbitrarily deep value returns an error rather than overflowing the
    /// stack as the value is released.
    ///
    /// # Errors
    ///
    /// Returns [`PropertyMapError::DuplicateName`] if two properties share a
    /// name, or [`PropertyMapError::DepthExceeded`] if one nests deeper than
    /// [`MAX_DEPTH`](Self::MAX_DEPTH).
    pub fn new(mut properties: Vec<Property>) -> Result<Self, PropertyMapError> {
        if let Some(duplicate) =
            sort_and_find_duplicate(&mut properties, |left, right| left.name.cmp(&right.name))
        {
            let error = PropertyMapError::DuplicateName(duplicate.name.clone());
            dismantle(properties);
            return Err(error);
        }
        if let Some(deep) = properties
            .iter()
            .find(|property| !within_depth(&property.value, Self::MAX_DEPTH))
        {
            let error = PropertyMapError::DepthExceeded {
                name: deep.name.clone(),
                limit: Self::MAX_DEPTH,
            };
            dismantle(properties);
            return Err(error);
        }
        Ok(Self::from_sorted(properties))
    }
}

sorted_collection!(PropertyMap, PropertyEntries, Property, name, PropertyValue);

/// Releases rejected properties without recursing through their nesting.
///
/// A value only reaches this after failing validation, so it can be deeper
/// than [`PropertyMap::MAX_DEPTH`] and deep enough that the recursive drop
/// glue would exhaust the stack. Moving each level onto a work list keeps the
/// teardown flat.
///
/// The variants matched here are the recursive ones, and must stay in step
/// with [`within_depth`]: a variant one walks and the other skips is either
/// unbounded nesting or an unbounded drop.
fn dismantle(properties: Vec<Property>) {
    let mut pending: Vec<PropertyValue> = properties
        .into_iter()
        .map(|property| property.value)
        .collect();

    while let Some(value) = pending.pop() {
        // Only arrays need flattening. An `Object` holds a `PropertyMap`,
        // which exists only by passing the depth check, so its own nesting
        // is already bounded and the recursive drop of one is safe.
        if let PropertyValue::Array(items) = value {
            pending.extend(items.into_vec());
        }
    }
}

/// Returns whether a value nests no deeper than `remaining` further levels.
///
/// The walk stops when the budget runs out, so its own recursion is bounded by
/// the limit even when the inspected value is deeper.
fn within_depth(value: &PropertyValue, remaining: u32) -> bool {
    match value {
        PropertyValue::Array(items) => {
            remaining > 0 && items.iter().all(|item| within_depth(item, remaining - 1))
        }
        PropertyValue::Object(map) => {
            remaining > 0
                && map
                    .iter()
                    .all(|property| within_depth(&property.value, remaining - 1))
        }
        _ => true,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PropertyMapError {
    DuplicateName(Name),
    DepthExceeded { name: Name, limit: u32 },
}

impl fmt::Display for PropertyMapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateName(name) => write!(formatter, "duplicate property name '{name}'"),
            Self::DepthExceeded { name, limit } => write!(
                formatter,
                "property '{name}' nests deeper than the limit of {limit} levels"
            ),
        }
    }
}

impl Error for PropertyMapError {}

/// Format-aware encoding for opaque byte properties.
///
/// Hex for human-readable formats, native bytes otherwise. Serde's default
/// sequence of integers is unreadable in a text format, and hex would double
/// the payload in a binary one.
#[cfg(feature = "serde")]
mod hex_bytes {
    use std::fmt;
    use std::sync::Arc;

    const DIGITS: &[u8; 16] = b"0123456789abcdef";

    pub(super) fn serialize<S>(value: &Arc<[u8]>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if !serializer.is_human_readable() {
            return serializer.serialize_bytes(value);
        }

        let mut encoded = String::with_capacity(value.len() * 2);
        for byte in value.iter() {
            encoded.push(DIGITS[usize::from(byte >> 4)] as char);
            encoded.push(DIGITS[usize::from(byte & 0x0f)] as char);
        }
        serializer.serialize_str(&encoded)
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Arc<[u8]>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        if !deserializer.is_human_readable() {
            return deserializer.deserialize_bytes(BytesVisitor);
        }

        let encoded = <String as serde::Deserialize>::deserialize(deserializer)?;
        if encoded.len() % 2 != 0 {
            return Err(D::Error::custom(
                "hex encoded bytes must have an even length",
            ));
        }

        let bytes = encoded.as_bytes();
        let mut decoded = Vec::with_capacity(bytes.len() / 2);
        for pair in bytes.chunks_exact(2) {
            let high = decode_digit::<D>(pair[0])?;
            let low = decode_digit::<D>(pair[1])?;
            decoded.push(high << 4 | low);
        }
        Ok(Arc::from(decoded))
    }

    /// Accepts whichever shape a binary format uses for a byte string.
    struct BytesVisitor;

    impl<'de> serde::de::Visitor<'de> for BytesVisitor {
        type Value = Arc<[u8]>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a byte string")
        }

        fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(Arc::from(value))
        }

        fn visit_byte_buf<E>(self, value: Vec<u8>) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(Arc::from(value))
        }

        fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            let mut bytes = Vec::with_capacity(sequence.size_hint().unwrap_or_default());
            while let Some(byte) = sequence.next_element::<u8>()? {
                bytes.push(byte);
            }
            Ok(Arc::from(bytes))
        }
    }

    fn decode_digit<'de, D>(digit: u8) -> Result<u8, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        match digit {
            b'0'..=b'9' => Ok(digit - b'0'),
            b'a'..=b'f' => Ok(digit - b'a' + 10),
            b'A'..=b'F' => Ok(digit - b'A' + 10),
            _ => Err(D::Error::custom(format!(
                "invalid hex digit '{}'",
                digit as char
            ))),
        }
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for PropertyMap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let properties = <Vec<Property> as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(properties).map_err(serde::de::Error::custom)
    }
}
