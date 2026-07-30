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
use super::value::value_conversions;
use super::Attributes;
use super::Finite;
use super::SourceKey;
use super::Subject;
use super::Timestamp;

/// Numeric reading value without forcing all integer data through `f64`.
///
/// The variant is part of the value's identity: `I64(1)`, `U64(1)`, and
/// `F64(1.0)` are unequal to each other and hash differently. A producer must
/// therefore choose one variant per signal and keep it, or a consumer diffing
/// successive observations reads a variant change as a value change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[non_exhaustive]
pub enum NumericValue {
    I64(i64),
    U64(u64),
    F64(Finite),
}

value_conversions!(numeric NumericValue);

/// How a reading's value evolves over time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[non_exhaustive]
pub enum ReadingKind {
    Gauge,
    Counter,
}

name_newtype!(
    /// Source-reported unit vocabulary.
    Unit
);

name_newtype!(
    /// What is being measured, such as `temperature`.
    Metric
);

name_newtype!(
    /// Which occurrence of a repeated observation this is, such as `CPU0Temp`.
    ///
    /// It distinguishes readings of one [`Metric`] and states of one
    /// [`StateName`](super::StateName) on the same subject; it never names
    /// what is being observed.
    Instance
);

name_newtype!(
    /// The source's own word for whether a subject is operating, such as
    /// `enabled` or `absent`.
    ///
    /// Deliberately not ordered, for the reason given on
    /// [`Severity`](super::Severity): the vocabulary is the source's, so a
    /// derived comparison would rank these alphabetically while reading as
    /// though it ranked availability.
    OperatingState,
    unordered
);

name_newtype!(
    /// The source's own judgement of a subject's condition, such as `ok`.
    ///
    /// Deliberately not ordered, for the reason given on
    /// [`Severity`](super::Severity). Redfish reports exactly `ok`,
    /// `warning`, and `critical`, so a derived comparison would put
    /// `critical` below `ok` below `warning` while reading as though it
    /// compared condition. Consumers needing a ranking map these onto their
    /// own scale.
    Health,
    unordered
);

/// State and health values reported by the device itself.
///
/// Both halves are optional and independent: a source may report either, both,
/// or neither, and there is no ordering between them to validate.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
#[non_exhaustive]
pub struct ReportedState {
    pub state: Option<OperatingState>,
    pub health: Option<Health>,
}

impl ReportedState {
    pub fn new(state: Option<OperatingState>, health: Option<Health>) -> Self {
        Self { state, health }
    }

    /// Returns whether the source reported neither half.
    pub const fn is_empty(&self) -> bool {
        self.state.is_none() && self.health.is_none()
    }
}

/// An optional lower and upper limit on a reading's value.
///
/// Either edge may be absent: a source that reports only a ceiling leaves the
/// floor unset rather than inventing one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "ValueRangeParts"))]
pub struct ValueRange {
    lower: Option<Finite>,
    upper: Option<Finite>,
}

impl ValueRange {
    /// Builds a range from two independently optional edges.
    ///
    /// Not public: the two arguments have the same type, so `new(Some(x),
    /// None)` and `new(None, Some(x))` both succeed and mean opposite things.
    /// [`between`](Self::between), [`at_least`](Self::at_least),
    /// [`at_most`](Self::at_most), and [`empty`](Self::empty) cover every
    /// combination and each name which edge it is given. This exists only for
    /// the decoding path below, where the edges arrive as named fields, and is
    /// compiled only with it.
    ///
    /// # Errors
    ///
    /// Returns [`RangeOrderError`] when both edges are present and `lower`
    /// exceeds `upper`.
    #[cfg(feature = "serde")]
    pub(crate) fn new(
        lower: Option<Finite>,
        upper: Option<Finite>,
    ) -> Result<Self, RangeOrderError> {
        match (lower, upper) {
            (Some(lower), Some(upper)) => Self::between(lower, upper),
            (Some(lower), None) => Ok(Self::at_least(lower)),
            (None, Some(upper)) => Ok(Self::at_most(upper)),
            (None, None) => Ok(Self::empty()),
        }
    }

    pub const fn empty() -> Self {
        Self {
            lower: None,
            upper: None,
        }
    }

    pub const fn at_least(lower: Finite) -> Self {
        Self {
            lower: Some(lower),
            upper: None,
        }
    }

    pub const fn at_most(upper: Finite) -> Self {
        Self {
            lower: None,
            upper: Some(upper),
        }
    }

    /// Builds a closed range, rejecting inverted bounds.
    ///
    /// # Errors
    ///
    /// Returns [`RangeOrderError`] when `lower` exceeds `upper`.
    pub fn between(lower: Finite, upper: Finite) -> Result<Self, RangeOrderError> {
        if lower > upper {
            return Err(RangeOrderError { lower, upper });
        }
        Ok(Self {
            lower: Some(lower),
            upper: Some(upper),
        })
    }

    pub const fn lower(self) -> Option<Finite> {
        self.lower
    }

    pub const fn upper(self) -> Option<Finite> {
        self.upper
    }

    pub const fn is_empty(self) -> bool {
        self.lower.is_none() && self.upper.is_none()
    }
}

/// The unvalidated edge pair a [`ValueRange`] is built from.
#[cfg(feature = "serde")]
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ValueRangeParts {
    lower: Option<Finite>,
    upper: Option<Finite>,
}

#[cfg(feature = "serde")]
impl TryFrom<ValueRangeParts> for ValueRange {
    type Error = RangeOrderError;

    fn try_from(value: ValueRangeParts) -> Result<Self, Self::Error> {
        Self::new(value.lower, value.upper)
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
/// A signal is a [`Metric`] at an [`Instance`], and [`new`](Self::new) takes
/// each concretely so that the pair cannot be given in the wrong order. Each
/// is built from a string in the same expression that passes it:
///
/// ```
/// use nv_telemetry_core::{
///     Instance, Metric, ReadingKind, SignalDescriptor, Subject, SubjectId,
///     SubjectKind, Timestamp, Unit,
/// };
///
/// let descriptor = SignalDescriptor::new(
///     Subject::new(SubjectKind::from_static("sensor"), SubjectId::from_static("CPU0Temp")),
///     Metric::from_static("temperature"),
///     Instance::from_static("CPU0Temp"),
///     ReadingKind::Gauge,
///     Unit::from_static("Cel"),
///     Timestamp::new(0, 0).unwrap(),
/// );
/// assert_eq!(descriptor.metric.as_str(), "temperature");
/// ```
///
/// Swapping the two stops compiling, inline or through variables, rather than
/// producing a signal that joins to the wrong readings:
///
/// ```compile_fail
/// use nv_telemetry_core::{
///     Instance, Metric, ReadingKind, SignalDescriptor, Subject, SubjectId,
///     SubjectKind, Timestamp, Unit,
/// };
///
/// let metric = Metric::from("temperature");
/// let instance = Instance::from("CPU0Temp");
/// let descriptor = SignalDescriptor::new(
///     Subject::new(SubjectKind::from_static("sensor"), SubjectId::from_static("CPU0Temp")),
///     instance,
///     metric,
///     ReadingKind::Gauge,
///     Unit::from_static("Cel"),
///     Timestamp::new(0, 0).unwrap(),
/// );
/// ```
///
/// [`matches_definition`]: SignalDescriptor::matches_definition
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
#[non_exhaustive]
pub struct SignalDescriptor {
    pub subject: Subject,
    pub metric: Metric,
    pub instance: Instance,
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
        metric: Metric,
        instance: Instance,
        kind: ReadingKind,
        unit: Unit,
        observed_at: Timestamp,
    ) -> Self {
        Self {
            subject,
            metric,
            instance,
            kind,
            unit,
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
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
#[non_exhaustive]
pub struct Reading {
    pub source_key: SourceKey,
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
        source_key: SourceKey,
        signal: impl Into<Arc<SignalDescriptor>>,
        value: impl Into<NumericValue>,
    ) -> Self {
        Self {
            source_key,
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

#[cfg(all(test, feature = "serde"))]
mod tests {
    use super::Finite;
    use super::ValueRange;

    /// The decoding constructor must agree with the named public ones.
    ///
    /// It is the only caller that supplies the edges as a same-typed pair, so
    /// nothing outside this module can get them the wrong way round.
    #[test]
    fn the_optional_edge_pair_agrees_with_the_named_constructors() {
        let lower = Finite::new(-5.0).expect("finite");
        let upper = Finite::new(100.0).expect("finite");

        assert_eq!(
            ValueRange::new(Some(lower), Some(upper)).expect("ordered edges"),
            ValueRange::between(lower, upper).expect("ordered edges")
        );
        assert_eq!(
            ValueRange::new(Some(lower), None).expect("one edge"),
            ValueRange::at_least(lower)
        );
        assert_eq!(
            ValueRange::new(None, Some(upper)).expect("one edge"),
            ValueRange::at_most(upper)
        );
        assert_eq!(
            ValueRange::new(None, None).expect("no edges"),
            ValueRange::empty()
        );
        assert!(ValueRange::new(Some(upper), Some(lower)).is_err());
    }
}
