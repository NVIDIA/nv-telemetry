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

use super::Attributes;
use super::Name;

/// Stable identity of an endpoint.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct EndpointId(Name);

impl EndpointId {
    pub const fn from_static(value: &'static str) -> Self {
        Self(Name::from_static(value))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl From<Name> for EndpointId {
    fn from(value: Name) -> Self {
        Self(value)
    }
}

impl From<String> for EndpointId {
    fn from(value: String) -> Self {
        Self(value.into())
    }
}

impl From<&str> for EndpointId {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

/// Immutable endpoint identity and non-secret static attributes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct EndpointContext {
    pub id: EndpointId,
    pub attributes: Attributes,
}

impl EndpointContext {
    pub fn new(id: impl Into<EndpointId>, attributes: Attributes) -> Self {
        Self {
            id: id.into(),
            attributes,
        }
    }
}

/// Stable, protocol-neutral identity of a resource inside an endpoint.
///
/// Fields are public; read and match them directly:
///
/// ```
/// use nv_telemetry_core::Subject;
///
/// let subject = Subject::new("sensor", "CPU0Temp");
/// assert_eq!(subject.kind.as_str(), "sensor");
///
/// let Subject { id, .. } = &subject;
/// assert_eq!(id.as_str(), "CPU0Temp");
/// ```
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Subject {
    pub kind: Name,
    pub id: Name,
}

impl Subject {
    pub fn new(kind: impl Into<Name>, id: impl Into<Name>) -> Self {
        Self {
            kind: kind.into(),
            id: id.into(),
        }
    }
}

/// The resource set for which completeness is asserted.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ObservationScope {
    Endpoint,
    Subject(Subject),
}

/// Whether all observations in a declared scope were obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum Completeness {
    Complete,
    Partial {
        /// Number known to have been omitted, when the source can determine it.
        omitted: Option<u64>,
    },
}

impl Completeness {
    pub const fn partial(omitted: Option<u64>) -> Self {
        Self::Partial { omitted }
    }

    pub const fn is_complete(self) -> bool {
        matches!(self, Self::Complete)
    }
}

/// Scope-relative completeness of an observation batch.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Coverage {
    pub scope: ObservationScope,
    pub completeness: Completeness,
}

impl Coverage {
    pub const fn new(scope: ObservationScope, completeness: Completeness) -> Self {
        Self {
            scope,
            completeness,
        }
    }

    pub const fn complete_endpoint() -> Self {
        Self::new(ObservationScope::Endpoint, Completeness::Complete)
    }

    pub fn complete_subject(subject: Subject) -> Self {
        Self::new(ObservationScope::Subject(subject), Completeness::Complete)
    }
}

/// Provenance of an acquisition.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Origin {
    pub provider: Name,
    pub request_class: Name,
}

impl Origin {
    pub fn new(provider: impl Into<Name>, request_class: impl Into<Name>) -> Self {
        Self {
            provider: provider.into(),
            request_class: request_class.into(),
        }
    }
}
