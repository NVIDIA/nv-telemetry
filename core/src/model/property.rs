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

use super::collection::sorted_collection;
use super::value::value_conversions;
use super::DurationValue;
use super::Finite;
use super::Name;
use super::SourceKey;
use super::Subject;
use super::Timestamp;

/// A source reference that may or may not resolve to a canonical subject.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
#[non_exhaustive]
pub struct ResourceReference {
    pub source_key: SourceKey,
    pub subject: Option<Subject>,
}

impl ResourceReference {
    pub fn new(source_key: SourceKey) -> Self {
        Self {
            source_key,
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
/// The encoding is adjacently tagged so every variant has the same shape.
/// Decoding accepts that pair however a format presents it: as a two-entry map
/// keyed by `type` and `value`, in either order, as JSON and CBOR write it, or
/// as a two-element sequence, as a format that writes structs compactly does.
///
/// [`ResourceCompleteness::Complete`]: super::ResourceCompleteness::Complete
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
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
    String(Name),
    #[cfg_attr(feature = "serde", serde(with = "hex_bytes"))]
    Bytes(Arc<[u8]>),
    Timestamp(Timestamp),
    Duration(DurationValue),
    Reference(ResourceReference),
    Array(PropertyArray),
    Object(PropertyMap),
}

value_conversions!(scalar PropertyValue);

impl PropertyValue {
    /// Builds an array property, rejecting excessive nesting.
    ///
    /// # Errors
    ///
    /// Returns [`PropertyArrayError::DepthExceeded`] if the array nests deeper
    /// than [`PropertyMap::MAX_DEPTH`].
    pub fn array(values: Vec<Self>) -> Result<Self, PropertyArrayError> {
        PropertyArray::new(values).map(Self::Array)
    }

    /// Returns whether this value nests no deeper than `remaining` further
    /// levels.
    ///
    /// The walk stops when the budget runs out, so its own recursion is
    /// bounded by the limit even when the inspected value is deeper.
    fn within_depth(&self, remaining: u32) -> bool {
        match self {
            Self::Array(items) => {
                remaining > 0 && items.iter().all(|item| item.within_depth(remaining - 1))
            }
            Self::Object(map) => {
                remaining > 0
                    && map
                        .iter()
                        .all(|property| property.value.within_depth(remaining - 1))
            }
            Self::Null
            | Self::Bool(_)
            | Self::I64(_)
            | Self::U64(_)
            | Self::F64(_)
            | Self::String(_)
            | Self::Bytes(_)
            | Self::Timestamp(_)
            | Self::Duration(_)
            | Self::Reference(_) => true,
        }
    }
}

/// A recursively bounded property array.
///
/// The wrapper keeps the recursive [`PropertyValue`] enum safe to clone, hash,
/// compare, serialize, and drop: every public construction path checks the
/// same depth bound as [`PropertyMap`]. Its serde representation is the
/// underlying sequence, preserving the wire shape of `PropertyValue::Array`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "Vec<PropertyValue>"))]
pub struct PropertyArray(Box<[PropertyValue]>);

impl PropertyArray {
    /// Builds an array, rejecting excessive nesting.
    ///
    /// # Errors
    ///
    /// Returns [`PropertyArrayError::DepthExceeded`] if the array nests deeper
    /// than [`PropertyMap::MAX_DEPTH`].
    pub fn new(values: Vec<PropertyValue>) -> Result<Self, PropertyArrayError> {
        let child_depth = PropertyMap::MAX_DEPTH - 1;
        if values.iter().any(|value| !value.within_depth(child_depth)) {
            return Err(PropertyArrayError::DepthExceeded {
                limit: PropertyMap::MAX_DEPTH,
            });
        }
        Ok(Self(values.into_boxed_slice()))
    }

    pub fn as_slice(&self) -> &[PropertyValue] {
        &self.0
    }

    pub fn iter(&self) -> std::slice::Iter<'_, PropertyValue> {
        self.0.iter()
    }

    pub const fn len(&self) -> usize {
        self.0.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_vec(self) -> Vec<PropertyValue> {
        self.0.into_vec()
    }
}

impl TryFrom<Vec<PropertyValue>> for PropertyArray {
    type Error = PropertyArrayError;

    fn try_from(values: Vec<PropertyValue>) -> Result<Self, Self::Error> {
        Self::new(values)
    }
}

impl AsRef<[PropertyValue]> for PropertyArray {
    fn as_ref(&self) -> &[PropertyValue] {
        self.as_slice()
    }
}

impl<'a> IntoIterator for &'a PropertyArray {
    type Item = &'a PropertyValue;
    type IntoIter = std::slice::Iter<'a, PropertyValue>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// A rejected recursively nested property array.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PropertyArrayError {
    DepthExceeded { limit: u32 },
}

impl fmt::Display for PropertyArrayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DepthExceeded { limit } => {
                write!(
                    formatter,
                    "property array nests deeper than the limit of {limit} levels"
                )
            }
        }
    }
}

impl Error for PropertyArrayError {}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for PropertyValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        property_value_depth::enter(deserializer)
    }
}

#[cfg(feature = "serde")]
mod property_value_depth {
    use std::cell::Cell;
    use std::fmt;

    use serde::de::DeserializeSeed as _;
    use serde::Deserialize as _;

    use super::PropertyArray;
    use super::PropertyMap;
    use super::PropertyValue;

    // When an adjacent tag follows its content, Serde has to buffer the
    // untyped content before it can select a variant. One recursive property
    // level uses at most four map/sequence levels (an object is the widest
    // case), and the fixed margin covers the leaf payload and envelopes.
    //
    // This is deliberately separate from `MAX_DEPTH`: it bounds work done
    // before the semantic property depth is knowable, while `DEPTH` below
    // remains the exact model invariant.
    const MAX_BUFFER_DEPTH: u32 = PropertyMap::MAX_DEPTH * 4 + 16;
    const MAX_BUFFER_PREALLOCATED_ITEMS: usize = 4096;

    std::thread_local! {
        static DEPTH: Cell<u32> = const { Cell::new(0) };
    }

    pub(super) fn enter<'de, D>(deserializer: D) -> Result<PropertyValue, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        DEPTH.with(|depth| {
            let current = depth.get();
            if current > PropertyMap::MAX_DEPTH {
                return Err(serde::de::Error::custom(format_args!(
                    "property value nests deeper than the limit of {} levels",
                    PropertyMap::MAX_DEPTH
                )));
            }

            depth.set(current + 1);
            let _restore = RestoreDepth { depth, current };
            let human_readable = deserializer.is_human_readable();
            deserializer.deserialize_struct(
                "PropertyValue",
                &["type", "value"],
                PropertyValueVisitor { human_readable },
            )
        })
    }

    struct RestoreDepth<'a> {
        depth: &'a Cell<u32>,
        current: u32,
    }

    impl Drop for RestoreDepth<'_> {
        fn drop(&mut self) {
            self.depth.set(self.current);
        }
    }

    /// Declares each wire tag with the variant it names and the type its
    /// payload decodes as.
    ///
    /// Every table the decoder needs is generated from this one: the tag type,
    /// the names an unrecognised tag is reported against, and the three places
    /// a payload is read from. A variant added to [`PropertyValue`] without an
    /// entry here fails to compile in `PropertyTag::of`, which is compiled in
    /// ordinary builds and not only test ones for exactly that reason; the
    /// tests below use it to tie the table to the tags the encoder derives.
    macro_rules! property_tags {
        ($($tag:literal => $variant:ident: $payload:ty => $build:expr),+ $(,)?) => {
            #[derive(Clone, Copy)]
            enum PropertyTag {
                $($variant,)+
            }

            impl PropertyTag {
                const NAMES: &'static [&'static str] = &[$($tag,)+];

                fn parse<E>(value: &str) -> Result<Self, E>
                where
                    E: serde::de::Error,
                {
                    match value {
                        $($tag => Ok(Self::$variant),)+
                        _ => Err(E::unknown_variant(value, Self::NAMES)),
                    }
                }

                /// Names the tag a value encodes under.
                ///
                /// The match is what makes a variant added to
                /// `PropertyValue` without a table entry a compile error, so
                /// it is compiled even though only the tests call it. Behind
                /// `cfg(test)` the check would not run for `cargo build` or
                /// `cargo publish`, leaving both to accept an encoder that
                /// emits a tag its own decoder rejects.
                #[allow(dead_code)]
                fn of(value: &PropertyValue) -> Self {
                    match value {
                        $(PropertyValue::$variant { .. } => Self::$variant,)+
                    }
                }

                #[cfg(test)]
                const fn name(self) -> &'static str {
                    match self {
                        $(Self::$variant => $tag,)+
                    }
                }
            }

            fn deserialize_tagged_value<'de, A>(
                tag: PropertyTag,
                map: &mut A,
            ) -> Result<PropertyValue, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                match tag {
                    $(PropertyTag::$variant => map.next_value::<$payload>().map($build),)+
                }
            }

            fn deserialize_tagged_element<'de, A>(
                tag: PropertyTag,
                sequence: &mut A,
            ) -> Result<Option<PropertyValue>, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                match tag {
                    $(PropertyTag::$variant => Ok(
                        sequence.next_element::<$payload>()?.map($build)
                    ),)+
                }
            }

            fn deserialize_buffered<E>(
                tag: PropertyTag,
                buffered: BufferedValue,
                human_readable: bool,
            ) -> Result<PropertyValue, E>
            where
                E: serde::de::Error,
            {
                let deserializer = BufferedDeserializer::<E>::new(buffered, human_readable);
                match tag {
                    $(PropertyTag::$variant => {
                        <$payload>::deserialize(deserializer).map($build)
                    })+
                }
            }
        };
    }

    property_tags! {
        "null" => Null: () => |()| PropertyValue::Null,
        "bool" => Bool: bool => PropertyValue::Bool,
        "i64" => I64: i64 => PropertyValue::I64,
        "u64" => U64: u64 => PropertyValue::U64,
        "f64" => F64: super::Finite => PropertyValue::F64,
        "string" => String: super::Name => PropertyValue::String,
        "bytes" => Bytes: BytesValue => |value: BytesValue| PropertyValue::Bytes(value.0),
        "timestamp" => Timestamp: super::Timestamp => PropertyValue::Timestamp,
        "duration" => Duration: super::DurationValue => PropertyValue::Duration,
        "reference" => Reference: super::ResourceReference => PropertyValue::Reference,
        "array" => Array: PropertyArray => PropertyValue::Array,
        "object" => Object: PropertyMap => PropertyValue::Object,
    }

    impl<'de> serde::Deserialize<'de> for PropertyTag {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            struct TagVisitor;

            impl serde::de::Visitor<'_> for TagVisitor {
                type Value = PropertyTag;

                fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                    formatter.write_str("a property value type")
                }

                fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                where
                    E: serde::de::Error,
                {
                    PropertyTag::parse(value)
                }
            }

            deserializer.deserialize_str(TagVisitor)
        }
    }

    #[derive(serde::Deserialize)]
    #[serde(field_identifier, rename_all = "snake_case")]
    enum PropertyField {
        Type,
        Value,
    }

    #[derive(serde::Deserialize)]
    #[serde(transparent)]
    struct BytesValue(#[serde(with = "super::hex_bytes")] std::sync::Arc<[u8]>);

    /// The `value` field, whose shape the tag's position decides.
    ///
    /// A tag already read admits the typed value straight away; a tag still to
    /// come leaves only the untyped buffer. One field cannot be both, which is
    /// why they share a slot.
    enum Content {
        Decoded(PropertyValue),
        Buffered(BufferedValue),
    }

    struct PropertyValueVisitor {
        human_readable: bool,
    }

    impl<'de> serde::de::Visitor<'de> for PropertyValueVisitor {
        type Value = PropertyValue;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an adjacently tagged property value")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            let mut tag = None;
            let mut content = None;

            while let Some(field) = map.next_key::<PropertyField>()? {
                match field {
                    PropertyField::Type => {
                        if tag.is_some() {
                            return Err(serde::de::Error::duplicate_field("type"));
                        }
                        tag = Some(map.next_value::<PropertyTag>()?);
                    }
                    PropertyField::Value => {
                        if content.is_some() {
                            return Err(serde::de::Error::duplicate_field("value"));
                        }
                        content = Some(match tag {
                            Some(tag) => Content::Decoded(deserialize_tagged_value(tag, &mut map)?),
                            None => Content::Buffered(map.next_value_seed(BufferSeed::root())?),
                        });
                    }
                }
            }

            let tag = tag.ok_or_else(|| serde::de::Error::missing_field("type"))?;
            match content {
                Some(Content::Decoded(value)) => Ok(value),
                Some(Content::Buffered(buffered)) => {
                    deserialize_buffered(tag, buffered, self.human_readable)
                }
                None if matches!(tag, PropertyTag::Null) => Ok(PropertyValue::Null),
                None => Err(serde::de::Error::missing_field("value")),
            }
        }

        fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            let tag = sequence
                .next_element::<PropertyTag>()?
                .ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;
            match deserialize_tagged_element(tag, &mut sequence)? {
                Some(value) => Ok(value),
                None if matches!(tag, PropertyTag::Null) => Ok(PropertyValue::Null),
                None => Err(serde::de::Error::invalid_length(1, &self)),
            }
        }
    }

    /// The subset of Serde's data model needed to replay an untyped `value`.
    ///
    /// Maps remain ordered entry vectors so duplicate fields survive replay
    /// and are rejected by the same validators as tag-first input.
    enum BufferedValue {
        Bool(bool),
        I64(i64),
        I128(i128),
        U64(u64),
        U128(u128),
        F64(f64),
        Char(char),
        String(String),
        Bytes(Vec<u8>),
        None,
        Some(Box<Self>),
        Unit,
        Newtype(Box<Self>),
        Sequence(Vec<Self>),
        Map(Vec<(Self, Self)>),
    }

    #[derive(Clone, Copy)]
    struct BufferSeed {
        remaining: u32,
    }

    impl BufferSeed {
        const fn root() -> Self {
            Self {
                remaining: MAX_BUFFER_DEPTH,
            }
        }

        fn descend<E>(self) -> Result<Self, E>
        where
            E: serde::de::Error,
        {
            self.remaining
                .checked_sub(1)
                .map(|remaining| Self { remaining })
                .ok_or_else(|| {
                    E::custom(format_args!(
                        "property value wire representation nests too deeply \
                         before its type tag (limit: {MAX_BUFFER_DEPTH} container levels)"
                    ))
                })
        }
    }

    impl<'de> serde::de::DeserializeSeed<'de> for BufferSeed {
        type Value = BufferedValue;

        fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            deserializer.deserialize_any(BufferVisitor {
                remaining: self.remaining,
            })
        }
    }

    struct BufferVisitor {
        remaining: u32,
    }

    impl<'de> serde::de::Visitor<'de> for BufferVisitor {
        type Value = BufferedValue;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a self-describing value")
        }

        fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
            Ok(BufferedValue::Bool(value))
        }

        fn visit_i8<E>(self, value: i8) -> Result<Self::Value, E> {
            Ok(BufferedValue::I64(i64::from(value)))
        }

        fn visit_i16<E>(self, value: i16) -> Result<Self::Value, E> {
            Ok(BufferedValue::I64(i64::from(value)))
        }

        fn visit_i32<E>(self, value: i32) -> Result<Self::Value, E> {
            Ok(BufferedValue::I64(i64::from(value)))
        }

        fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
            Ok(BufferedValue::I64(value))
        }

        fn visit_i128<E>(self, value: i128) -> Result<Self::Value, E> {
            Ok(BufferedValue::I128(value))
        }

        fn visit_u8<E>(self, value: u8) -> Result<Self::Value, E> {
            Ok(BufferedValue::U64(u64::from(value)))
        }

        fn visit_u16<E>(self, value: u16) -> Result<Self::Value, E> {
            Ok(BufferedValue::U64(u64::from(value)))
        }

        fn visit_u32<E>(self, value: u32) -> Result<Self::Value, E> {
            Ok(BufferedValue::U64(u64::from(value)))
        }

        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
            Ok(BufferedValue::U64(value))
        }

        fn visit_u128<E>(self, value: u128) -> Result<Self::Value, E> {
            Ok(BufferedValue::U128(value))
        }

        fn visit_f32<E>(self, value: f32) -> Result<Self::Value, E> {
            Ok(BufferedValue::F64(f64::from(value)))
        }

        fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
            Ok(BufferedValue::F64(value))
        }

        fn visit_char<E>(self, value: char) -> Result<Self::Value, E> {
            Ok(BufferedValue::Char(value))
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
            Ok(BufferedValue::String(value.to_owned()))
        }

        fn visit_borrowed_str<E>(self, value: &'de str) -> Result<Self::Value, E> {
            Ok(BufferedValue::String(value.to_owned()))
        }

        fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
            Ok(BufferedValue::String(value))
        }

        fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E> {
            Ok(BufferedValue::Bytes(value.to_vec()))
        }

        fn visit_borrowed_bytes<E>(self, value: &'de [u8]) -> Result<Self::Value, E> {
            Ok(BufferedValue::Bytes(value.to_vec()))
        }

        fn visit_byte_buf<E>(self, value: Vec<u8>) -> Result<Self::Value, E> {
            Ok(BufferedValue::Bytes(value))
        }

        fn visit_none<E>(self) -> Result<Self::Value, E> {
            Ok(BufferedValue::None)
        }

        fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            BufferSeed {
                remaining: self.remaining,
            }
            .deserialize(deserializer)
            .map(Box::new)
            .map(BufferedValue::Some)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E> {
            Ok(BufferedValue::Unit)
        }

        fn visit_newtype_struct<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            BufferSeed {
                remaining: self.remaining,
            }
            .deserialize(deserializer)
            .map(Box::new)
            .map(BufferedValue::Newtype)
        }

        fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            let seed = BufferSeed {
                remaining: self.remaining,
            }
            .descend()?;
            let mut values = Vec::with_capacity(
                sequence
                    .size_hint()
                    .unwrap_or_default()
                    .min(MAX_BUFFER_PREALLOCATED_ITEMS),
            );
            while let Some(value) = sequence.next_element_seed(seed)? {
                values.push(value);
            }
            Ok(BufferedValue::Sequence(values))
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            let seed = BufferSeed {
                remaining: self.remaining,
            }
            .descend()?;
            let mut values = Vec::with_capacity(
                map.size_hint()
                    .unwrap_or_default()
                    .min(MAX_BUFFER_PREALLOCATED_ITEMS),
            );
            while let Some(entry) = map.next_entry_seed(seed, seed)? {
                values.push(entry);
            }
            Ok(BufferedValue::Map(values))
        }
    }

    struct BufferedDeserializer<E> {
        value: BufferedValue,
        human_readable: bool,
        error: std::marker::PhantomData<E>,
    }

    impl<E> BufferedDeserializer<E> {
        const fn new(value: BufferedValue, human_readable: bool) -> Self {
            Self {
                value,
                human_readable,
                error: std::marker::PhantomData,
            }
        }
    }

    impl<'de, E> serde::Deserializer<'de> for BufferedDeserializer<E>
    where
        E: serde::de::Error,
    {
        type Error = E;

        fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
        where
            V: serde::de::Visitor<'de>,
        {
            let human_readable = self.human_readable;
            match self.value {
                BufferedValue::Bool(value) => visitor.visit_bool(value),
                BufferedValue::I64(value) => visitor.visit_i64(value),
                BufferedValue::I128(value) => visitor.visit_i128(value),
                BufferedValue::U64(value) => visitor.visit_u64(value),
                BufferedValue::U128(value) => visitor.visit_u128(value),
                BufferedValue::F64(value) => visitor.visit_f64(value),
                BufferedValue::Char(value) => visitor.visit_char(value),
                BufferedValue::String(value) => visitor.visit_string(value),
                BufferedValue::Bytes(value) => visitor.visit_byte_buf(value),
                BufferedValue::None => visitor.visit_none(),
                BufferedValue::Some(value) => visitor.visit_some(Self::new(*value, human_readable)),
                BufferedValue::Unit => visitor.visit_unit(),
                BufferedValue::Newtype(value) => {
                    visitor.visit_newtype_struct(Self::new(*value, human_readable))
                }
                BufferedValue::Sequence(values) => {
                    visitor.visit_seq(BufferedSequence::new(values, human_readable))
                }
                BufferedValue::Map(values) => {
                    visitor.visit_map(BufferedMap::new(values, human_readable))
                }
            }
        }

        fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, Self::Error>
        where
            V: serde::de::Visitor<'de>,
        {
            let human_readable = self.human_readable;
            match self.value {
                BufferedValue::None | BufferedValue::Unit => visitor.visit_none(),
                BufferedValue::Some(value) => visitor.visit_some(Self::new(*value, human_readable)),
                value => visitor.visit_some(Self::new(value, human_readable)),
            }
        }

        fn deserialize_newtype_struct<V>(
            self,
            _name: &'static str,
            visitor: V,
        ) -> Result<V::Value, Self::Error>
        where
            V: serde::de::Visitor<'de>,
        {
            let human_readable = self.human_readable;
            match self.value {
                BufferedValue::Newtype(value) => {
                    visitor.visit_newtype_struct(Self::new(*value, human_readable))
                }
                value => visitor.visit_newtype_struct(Self::new(value, human_readable)),
            }
        }

        fn is_human_readable(&self) -> bool {
            self.human_readable
        }

        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
            bytes byte_buf unit unit_struct seq tuple tuple_struct map struct enum identifier
            ignored_any
        }
    }

    struct BufferedSequence<E> {
        values: std::vec::IntoIter<BufferedValue>,
        human_readable: bool,
        error: std::marker::PhantomData<E>,
    }

    impl<E> BufferedSequence<E> {
        fn new(values: Vec<BufferedValue>, human_readable: bool) -> Self {
            Self {
                values: values.into_iter(),
                human_readable,
                error: std::marker::PhantomData,
            }
        }
    }

    impl<'de, E> serde::de::SeqAccess<'de> for BufferedSequence<E>
    where
        E: serde::de::Error,
    {
        type Error = E;

        fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error>
        where
            T: serde::de::DeserializeSeed<'de>,
        {
            self.values
                .next()
                .map(|value| {
                    seed.deserialize(BufferedDeserializer::new(value, self.human_readable))
                })
                .transpose()
        }

        fn size_hint(&self) -> Option<usize> {
            Some(self.values.len())
        }
    }

    struct BufferedMap<E> {
        entries: std::vec::IntoIter<(BufferedValue, BufferedValue)>,
        value: Option<BufferedValue>,
        human_readable: bool,
        error: std::marker::PhantomData<E>,
    }

    impl<E> BufferedMap<E> {
        fn new(values: Vec<(BufferedValue, BufferedValue)>, human_readable: bool) -> Self {
            Self {
                entries: values.into_iter(),
                value: None,
                human_readable,
                error: std::marker::PhantomData,
            }
        }
    }

    impl<'de, E> serde::de::MapAccess<'de> for BufferedMap<E>
    where
        E: serde::de::Error,
    {
        type Error = E;

        fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>, Self::Error>
        where
            K: serde::de::DeserializeSeed<'de>,
        {
            let Some((key, value)) = self.entries.next() else {
                return Ok(None);
            };
            self.value = Some(value);
            seed.deserialize(BufferedDeserializer::new(key, self.human_readable))
                .map(Some)
        }

        fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value, Self::Error>
        where
            V: serde::de::DeserializeSeed<'de>,
        {
            let value = self.value.take().ok_or_else(|| {
                E::custom("map value requested before its key was successfully decoded")
            })?;
            seed.deserialize(BufferedDeserializer::new(value, self.human_readable))
        }

        fn size_hint(&self) -> Option<usize> {
            Some(self.entries.len())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::PropertyTag;
        use super::PropertyValue;

        /// The tag table and the derived encoder must agree on every name.
        ///
        /// The encoder derives its tags from the variant names; the decoder
        /// reads them from the table. Nothing but this makes the two spellings
        /// the same string.
        #[test]
        fn every_variant_encodes_under_the_tag_the_table_decodes() {
            let samples = [
                PropertyValue::Null,
                PropertyValue::Bool(true),
                PropertyValue::I64(-1),
                PropertyValue::U64(1),
                PropertyValue::f64(1.5).expect("finite"),
                PropertyValue::from("text"),
                PropertyValue::Bytes(std::sync::Arc::from([0_u8].as_slice())),
                PropertyValue::Timestamp(super::super::Timestamp::new(0, 0).expect("valid")),
                PropertyValue::Duration(super::super::DurationValue::new(0, 0).expect("valid")),
                PropertyValue::Reference(super::super::ResourceReference::new(
                    super::super::SourceKey::from_static("/x"),
                )),
                PropertyValue::array(Vec::new()).expect("empty array"),
                PropertyValue::Object(super::PropertyMap::empty()),
            ];

            for sample in samples {
                let encoded = serde_json::to_value(&sample).expect("encodes");
                assert_eq!(
                    encoded["type"].as_str(),
                    Some(PropertyTag::of(&sample).name()),
                    "{sample:?}"
                );
            }
        }
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

impl From<PropertyArray> for PropertyValue {
    fn from(value: PropertyArray) -> Self {
        Self::Array(value)
    }
}

/// One named resource property.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
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
#[cfg_attr(feature = "serde", derive(serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "Vec<Property>"))]
pub struct PropertyMap(Arc<PropertyEntries>);

impl PropertyMap {
    /// Maximum number of levels a property value may nest through.
    ///
    /// A leaf nests through none and each enclosing array or object costs one,
    /// including the map a property is stored in. [`PropertyArray::new`] and
    /// [`new`](Self::new) hold what they produce to that bound and the decoder
    /// admits exactly that much, so a value this crate builds is one it can
    /// also read back.
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
    /// Input is already bounded before it arrives: an array carries
    /// [`PropertyArray`]'s check and an object carries a validated map, so the
    /// deepest value this can be handed is one level past what it accepts.
    /// Releasing a rejected one therefore recurses one level further than a
    /// stored one does, which is still bounded.
    ///
    /// # Errors
    ///
    /// Returns [`PropertyMapError::DuplicateName`] if two properties share a
    /// name, or [`PropertyMapError::DepthExceeded`] if one nests deeper than
    /// [`MAX_DEPTH`](Self::MAX_DEPTH).
    pub fn new(properties: Vec<Property>) -> Result<Self, PropertyMapError> {
        let properties = Self::sorted_unique(properties, |duplicate| {
            PropertyMapError::DuplicateName(duplicate.name.clone())
        })?;
        if let Some(deep) = properties
            .iter()
            .find(|property| !property.value.within_depth(Self::MAX_DEPTH - 1))
        {
            return Err(PropertyMapError::DepthExceeded {
                name: deep.name.clone(),
                limit: Self::MAX_DEPTH,
            });
        }
        Ok(Self::from_sorted(properties))
    }
}

sorted_collection!(
    PropertyMap,
    PropertyEntries,
    Property,
    name,
    PropertyValue,
    PropertyMapError
);

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
    const MAX_PREALLOCATED_BYTES: usize = 64 * 1024;

    pub(super) fn serialize<S>(value: &Arc<[u8]>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if !serializer.is_human_readable() {
            return serializer.serialize_bytes(value);
        }

        let capacity = value.len().checked_mul(2).ok_or_else(|| {
            serde::ser::Error::custom("hex encoded property length exceeds addressable capacity")
        })?;
        let mut encoded = String::new();
        encoded.try_reserve_exact(capacity).map_err(|error| {
            serde::ser::Error::custom(format_args!(
                "hex encoded property cannot reserve {capacity} bytes: {error}"
            ))
        })?;
        for byte in value.iter() {
            encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
            encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
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

        // Collected by hand rather than with `collect`, which cannot size the
        // output: a fallible iterator reports no lower bound, so the buffer
        // would grow from empty even though the length is known exactly.
        //
        // The reservation needs no cap of its own, unlike the ones taken from
        // a size hint below: it is half of a string the decoder has already
        // allocated in full.
        let bytes = encoded.as_bytes();
        let mut decoded = Vec::with_capacity(bytes.len() / 2);
        for pair in bytes.chunks_exact(2) {
            decoded.push(decode_digit::<D>(pair[0])? << 4 | decode_digit::<D>(pair[1])?);
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
            let capacity = sequence
                .size_hint()
                .unwrap_or_default()
                .min(MAX_PREALLOCATED_BYTES);
            let mut bytes = Vec::with_capacity(capacity);
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
            _ => Err(D::Error::custom(format_args!(
                "invalid hex digit {digit:#04x}"
            ))),
        }
    }
}
