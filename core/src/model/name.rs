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

use std::borrow::Borrow;
use std::cmp::Ordering;
use std::fmt;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::Arc;

/// An immutable string used for model vocabulary and device-supplied names.
///
/// Static vocabulary can be stored without allocation. Device-supplied values
/// use shared ownership, but this type does not perform global interning.
#[derive(Clone)]
pub struct Name(NameRepr);

#[derive(Clone)]
enum NameRepr {
    Static(&'static str),
    Shared(Arc<str>),
}

impl Name {
    /// Creates an allocation-free name for fixed library vocabulary.
    pub const fn from_static(value: &'static str) -> Self {
        Self(NameRepr::Static(value))
    }

    /// Creates a name from shared string storage.
    pub fn from_shared(value: Arc<str>) -> Self {
        Self(NameRepr::Shared(value))
    }

    /// Returns the string content.
    pub fn as_str(&self) -> &str {
        match &self.0 {
            NameRepr::Static(value) => value,
            NameRepr::Shared(value) => value,
        }
    }

    /// Returns whether this name uses allocation-free static storage.
    pub const fn is_static(&self) -> bool {
        matches!(self.0, NameRepr::Static(_))
    }
}

impl From<String> for Name {
    fn from(value: String) -> Self {
        Self::from_shared(Arc::from(value))
    }
}

impl From<Box<str>> for Name {
    fn from(value: Box<str>) -> Self {
        Self::from_shared(Arc::from(value))
    }
}

impl From<Arc<str>> for Name {
    fn from(value: Arc<str>) -> Self {
        Self::from_shared(value)
    }
}

impl From<&str> for Name {
    fn from(value: &str) -> Self {
        Self::from_shared(Arc::from(value))
    }
}

impl AsRef<str> for Name {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for Name {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for Name {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Name").field(&self.as_str()).finish()
    }
}

impl PartialEq for Name {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for Name {}

impl PartialOrd for Name {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Name {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl Hash for Name {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for Name {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Name {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Ok(value.into())
    }
}
