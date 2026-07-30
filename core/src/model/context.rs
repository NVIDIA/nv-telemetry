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

use super::name::name_newtype;
use super::Attributes;
use super::Name;

/// Stable identity of an endpoint.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct EndpointId(Name);

name_newtype!(EndpointId);

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

/// What kind of thing a subject names, such as `sensor`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct SubjectKind(Name);

name_newtype!(SubjectKind);

/// Which thing of that kind a subject names, unique within its kind.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct SubjectId(Name);

name_newtype!(SubjectId);

/// Stable, protocol-neutral identity of a resource inside an endpoint.
///
/// Fields are public; read and match them directly:
///
/// ```
/// use nv_telemetry_core::{Subject, SubjectId, SubjectKind};
///
/// let subject = Subject::new(
///     SubjectKind::from_static("sensor"),
///     SubjectId::from_static("CPU0Temp"),
/// );
/// assert_eq!(subject.kind.as_str(), "sensor");
///
/// let Subject { id, .. } = &subject;
/// assert_eq!(id.as_str(), "CPU0Temp");
/// ```
///
/// The two components use distinct types, so swapping variables is rejected:
///
/// ```compile_fail
/// use nv_telemetry_core::{Subject, SubjectId, SubjectKind};
///
/// let kind = SubjectKind::from_static("sensor");
/// let id = SubjectId::from_static("CPU0Temp");
/// let subject = Subject::new(id, kind);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Subject {
    pub kind: SubjectKind,
    pub id: SubjectId,
}

impl Subject {
    pub const fn new(kind: SubjectKind, id: SubjectId) -> Self {
        Self { kind, id }
    }
}

/// A protocol-specific location from which an observation was obtained.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct SourceKey(Name);

name_newtype!(SourceKey);

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

/// Which acquisition source produced an observation, such as `redfish`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct Provider(Name);

name_newtype!(Provider);

/// Which category of request produced an observation, as the caller's
/// scheduling and rate policy names it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct RequestClass(Name);

name_newtype!(RequestClass);

/// Provenance of an acquisition.
///
/// Provider and request class are separate semantic types, so a swapped call
/// does not compile:
///
/// ```compile_fail
/// use nv_telemetry_core::{Origin, Provider, RequestClass};
///
/// let provider = Provider::from_static("redfish");
/// let request = RequestClass::from_static("sensor-poll");
/// let origin = Origin::new(request, provider);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Origin {
    pub provider: Provider,
    pub request_class: RequestClass,
}

impl Origin {
    pub const fn new(provider: Provider, request_class: RequestClass) -> Self {
        Self {
            provider,
            request_class,
        }
    }
}
