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
use super::name::name_newtype;
use super::Finite;
use super::Name;
use super::NonFiniteError;

/// An attribute key, distinct from its value at the type level.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct AttrKey(Name);

name_newtype!(AttrKey);

/// A typed scalar attribute value.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[non_exhaustive]
pub enum AttrValue {
    String(Name),
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(Finite),
}

impl AttrValue {
    /// Builds a floating point attribute, rejecting non-finite input.
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

impl From<Name> for AttrValue {
    fn from(value: Name) -> Self {
        Self::String(value)
    }
}

impl From<String> for AttrValue {
    fn from(value: String) -> Self {
        Self::String(value.into())
    }
}

impl From<&str> for AttrValue {
    fn from(value: &str) -> Self {
        Self::String(value.into())
    }
}

impl From<bool> for AttrValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for AttrValue {
    fn from(value: i64) -> Self {
        Self::I64(value)
    }
}

impl From<u64> for AttrValue {
    fn from(value: u64) -> Self {
        Self::U64(value)
    }
}

impl From<Finite> for AttrValue {
    fn from(value: Finite) -> Self {
        Self::F64(value)
    }
}

impl TryFrom<f64> for AttrValue {
    type Error = NonFiniteError;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::f64(value)
    }
}

/// One key-value attribute.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Attribute {
    pub key: AttrKey,
    pub value: AttrValue,
}

impl Attribute {
    pub fn new(key: impl Into<AttrKey>, value: impl Into<AttrValue>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

/// An immutable, key-sorted attribute collection.
#[derive(Clone)]
pub struct Attributes(Arc<AttributeEntries>);

impl Attributes {
    /// Sorts attributes by key and rejects duplicate keys.
    ///
    /// # Errors
    ///
    /// Returns [`AttributesError::DuplicateKey`] if two attributes share a key.
    pub fn new(mut attributes: Vec<Attribute>) -> Result<Self, AttributesError> {
        if let Some(duplicate) =
            sort_and_find_duplicate(&mut attributes, |left, right| left.key.cmp(&right.key))
        {
            return Err(AttributesError::DuplicateKey(duplicate.key.clone()));
        }
        Ok(Self::from_sorted(attributes))
    }
}

sorted_collection!(Attributes, AttributeEntries, Attribute, key, AttrValue);

impl TryFrom<Vec<Attribute>> for Attributes {
    type Error = AttributesError;

    fn try_from(value: Vec<Attribute>) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AttributesError {
    DuplicateKey(AttrKey),
}

impl fmt::Display for AttributesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateKey(key) => {
                write!(formatter, "duplicate attribute key '{}'", key.as_str())
            }
        }
    }
}

impl Error for AttributesError {}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Attributes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let attributes = <Vec<Attribute> as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(attributes).map_err(serde::de::Error::custom)
    }
}
