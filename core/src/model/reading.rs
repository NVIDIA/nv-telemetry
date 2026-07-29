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

use super::name::name_newtype;
use super::Attributes;
use super::Finite;
use super::Name;
use super::NonFiniteError;
use super::Subject;
use super::Timestamp;

/// Numeric reading value without forcing all integer data through `f64`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[non_exhaustive]
pub enum NumericValue {
    I64(i64),
    U64(u64),
    F64(Finite),
}

impl NumericValue {
    /// Builds a floating point reading, rejecting non-finite input.
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

impl From<i64> for NumericValue {
    fn from(value: i64) -> Self {
        Self::I64(value)
    }
}

impl From<u64> for NumericValue {
    fn from(value: u64) -> Self {
        Self::U64(value)
    }
}

impl From<Finite> for NumericValue {
    fn from(value: Finite) -> Self {
        Self::F64(value)
    }
}

impl TryFrom<f64> for NumericValue {
    type Error = NonFiniteError;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::f64(value)
    }
}

/// How a reading's value evolves over time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[non_exhaustive]
pub enum ReadingKind {
    Gauge,
    Counter,
}

/// Source-reported unit vocabulary.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct Unit(Name);

name_newtype!(Unit);

/// State and health values reported by the device itself.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ReportedState {
    pub state: Option<Name>,
    pub health: Option<Name>,
}

impl ReportedState {
    pub fn new(state: Option<Name>, health: Option<Name>) -> Self {
        Self { state, health }
    }
}

/// An optional lower and upper limit on a reading's value.
///
/// Either edge may be absent: a source that reports only a ceiling leaves the
/// floor unset rather than inventing one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ValueRange {
    pub lower: Option<Finite>,
    pub upper: Option<Finite>,
}

impl ValueRange {
    pub const fn new(lower: Option<Finite>, upper: Option<Finite>) -> Self {
        Self { lower, upper }
    }

    pub const fn empty() -> Self {
        Self::new(None, None)
    }

    #[must_use]
    pub const fn with_lower(mut self, value: Finite) -> Self {
        self.lower = Some(value);
        self
    }

    #[must_use]
    pub const fn with_upper(mut self, value: Finite) -> Self {
        self.upper = Some(value);
        self
    }

    pub const fn is_empty(self) -> bool {
        self.lower.is_none() && self.upper.is_none()
    }

    /// Verifies that the reported lower limit does not exceed the upper.
    ///
    /// Storage stays permissive; a projection calls this and drops a
    /// contradictory range rather than publishing one.
    ///
    /// # Errors
    ///
    /// Returns [`RangeOrderError`] if the two edges contradict each other.
    pub fn checked(self) -> Result<Self, RangeOrderError> {
        match (self.lower, self.upper) {
            (Some(lower), Some(upper)) if lower > upper => Err(RangeOrderError { lower, upper }),
            _ => Ok(self),
        }
    }
}

/// Two reported range edges that contradict their intended ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RangeOrderError {
    pub lower: Finite,
    pub upper: Finite,
}

impl fmt::Display for RangeOrderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "lower limit of {} must not exceed upper limit of {}",
            self.lower, self.upper
        )
    }
}

impl Error for RangeOrderError {}

/// Immutable metadata shared by readings of one logical signal.
///
/// `observed_at` records when this definition was first observed and
/// `revision` counts how many times it has changed. Neither advances on a
/// refresh reporting identical content; see [`matches_definition`].
///
/// [`matches_definition`]: SignalDescriptor::matches_definition
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct SignalDescriptor {
    pub subject: Subject,
    pub metric: Name,
    pub instance: Name,
    pub kind: ReadingKind,
    pub unit: Unit,
    /// When this definition was first observed.
    pub observed_at: Timestamp,
    /// How many times the definition has changed.
    pub revision: u64,
    pub attributes: Attributes,
    pub bounds: Option<ValueRange>,
}

impl SignalDescriptor {
    pub fn new(
        subject: Subject,
        metric: impl Into<Name>,
        instance: impl Into<Name>,
        kind: ReadingKind,
        unit: impl Into<Unit>,
        observed_at: Timestamp,
    ) -> Self {
        Self {
            subject,
            metric: metric.into(),
            instance: instance.into(),
            kind,
            unit: unit.into(),
            observed_at,
            revision: 0,
            attributes: Attributes::empty(),
            bounds: None,
        }
    }

    #[must_use]
    pub const fn with_revision(mut self, revision: u64) -> Self {
        self.revision = revision;
        self
    }

    #[must_use]
    pub fn with_attributes(mut self, attributes: Attributes) -> Self {
        self.attributes = attributes;
        self
    }

    #[must_use]
    pub fn with_bounds(mut self, bounds: ValueRange) -> Self {
        self.bounds = Some(bounds);
        self
    }

    /// Returns whether two descriptors define the same signal.
    ///
    /// Ignores `observed_at` and `revision`, which record when the definition
    /// was seen rather than what it says.
    pub fn matches_definition(&self, other: &Self) -> bool {
        // Destructured so a new field fails to compile here rather than
        // silently counting as unchanged.
        let Self {
            subject,
            metric,
            instance,
            kind,
            unit,
            observed_at: _,
            revision: _,
            attributes,
            bounds,
        } = self;

        subject == &other.subject
            && metric == &other.metric
            && instance == &other.instance
            && kind == &other.kind
            && unit == &other.unit
            && attributes == &other.attributes
            && bounds == &other.bounds
    }
}

/// One numeric observation associated with a shared signal descriptor.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Reading {
    pub source_key: Name,
    /// Metadata shared with every other reading of the same signal.
    ///
    /// `Arc::ptr_eq` is a fast path for "same signal", not a substitute for
    /// comparing descriptors: readings built from different catalogs hold
    /// equal definitions separately. Sharing survives a round trip through a
    /// [`Payload`]; a reading serialized alone comes back unshared.
    ///
    /// [`Payload`]: super::Payload
    pub signal: Arc<SignalDescriptor>,
    pub value: NumericValue,
    /// When the source sampled this value, if it reported a per-sample time.
    ///
    /// `None` attributes the reading to the batch's [`ObservationWindow`]; it
    /// never means the sample is undated. A time that is present is not
    /// constrained to that window and is often older, since a device reports
    /// when it last refreshed the sensor rather than when we asked.
    ///
    /// [`ObservationWindow`]: super::ObservationWindow
    pub observed_at: Option<Timestamp>,
    pub attributes: Attributes,
    pub reported_state: Option<ReportedState>,
}

impl Reading {
    /// Builds a reading against a signal definition.
    pub fn new(
        source_key: impl Into<Name>,
        signal: impl Into<Arc<SignalDescriptor>>,
        value: impl Into<NumericValue>,
    ) -> Self {
        Self {
            source_key: source_key.into(),
            signal: signal.into(),
            value: value.into(),
            observed_at: None,
            attributes: Attributes::empty(),
            reported_state: None,
        }
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

    #[must_use]
    pub fn with_reported_state(mut self, reported_state: ReportedState) -> Self {
        self.reported_state = Some(reported_state);
        self
    }
}
