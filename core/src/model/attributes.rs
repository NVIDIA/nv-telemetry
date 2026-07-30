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
use super::name::name_newtype;
use super::value::value_conversions;
use super::Finite;
use super::Name;

name_newtype!(
    /// An attribute key, distinct from its value at the type level.
    AttrKey
);

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

value_conversions!(scalar AttrValue);

/// One key-value attribute.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
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
#[cfg_attr(feature = "serde", derive(serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "Vec<Attribute>"))]
pub struct Attributes(Arc<AttributeEntries>);

impl Attributes {
    /// Sorts attributes by key and rejects duplicate keys.
    ///
    /// # Errors
    ///
    /// Returns [`AttributesError::DuplicateKey`] if two attributes share a key.
    pub fn new(attributes: Vec<Attribute>) -> Result<Self, AttributesError> {
        Self::sorted_unique(attributes, |duplicate| {
            AttributesError::DuplicateKey(duplicate.key.clone())
        })
        .map(Self::from_sorted)
    }
}

sorted_collection!(
    Attributes,
    AttributeEntries,
    Attribute,
    key,
    AttrValue,
    AttributesError
);

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
