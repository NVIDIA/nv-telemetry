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
use super::AttrValue;
use super::Attributes;
use super::Instance;
use super::Subject;
use super::Timestamp;

name_newtype!(
    /// Which state a source is reporting, such as `power_state`.
    StateName
);

/// One typed state observation reported by a source.
///
/// The name is a [`StateName`] rather than a string so that it cannot trade
/// places with the value, which accepts a string of its own.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
#[non_exhaustive]
pub struct StateObservation {
    pub name: StateName,
    pub instance: Option<Instance>,
    pub subject: Option<Subject>,
    pub value: AttrValue,
    pub observed_at: Option<Timestamp>,
    pub attributes: Attributes,
}

impl StateObservation {
    pub fn new(name: StateName, value: impl Into<AttrValue>) -> Self {
        Self {
            name,
            instance: None,
            subject: None,
            value: value.into(),
            observed_at: None,
            attributes: Attributes::empty(),
        }
    }

    #[must_use]
    pub fn with_instance(mut self, instance: impl Into<Instance>) -> Self {
        self.instance = Some(instance.into());
        self
    }

    #[must_use]
    pub fn with_subject(mut self, subject: Subject) -> Self {
        self.subject = Some(subject);
        self
    }

    #[must_use]
    pub fn with_observed_at(mut self, observed_at: Timestamp) -> Self {
        self.observed_at = Some(observed_at);
        self
    }

    #[must_use]
    pub fn with_attributes(mut self, attributes: Attributes) -> Self {
        self.attributes = attributes;
        self
    }
}
