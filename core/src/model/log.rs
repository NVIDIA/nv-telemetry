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

use std::sync::Arc;

use super::Attributes;
use super::Name;
use super::Subject;
use super::Timestamp;

/// Source-reported log severity.
///
/// Deliberately not ordered: the value is whatever vocabulary the source uses,
/// so a derived comparison would sort `critical` below `warning` while reading
/// as though it compared urgency. Consumers needing a ranking map these onto
/// their own scale.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct Severity(Name);

impl Severity {
    pub const fn from_static(value: &'static str) -> Self {
        Self(Name::from_static(value))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl From<Name> for Severity {
    fn from(value: Name) -> Self {
        Self(value)
    }
}

impl From<String> for Severity {
    fn from(value: String) -> Self {
        Self(value.into())
    }
}

impl From<&str> for Severity {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

/// One source log record.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct LogRecord {
    pub record_id: Option<Name>,
    pub subject: Option<Subject>,
    pub severity: Severity,
    pub message: Arc<str>,
    pub observed_at: Option<Timestamp>,
    pub attributes: Attributes,
}

impl LogRecord {
    pub fn new(severity: impl Into<Severity>, message: impl Into<Arc<str>>) -> Self {
        Self {
            record_id: None,
            subject: None,
            severity: severity.into(),
            message: message.into(),
            observed_at: None,
            attributes: Attributes::empty(),
        }
    }

    #[must_use]
    pub fn with_record_id(mut self, record_id: impl Into<Name>) -> Self {
        self.record_id = Some(record_id.into());
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
