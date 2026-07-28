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

use super::Coverage;
use super::EndpointContext;
use super::InventoryItem;
use super::LogRecord;
use super::ObservationScope;
use super::ObservationWindow;
use super::Origin;
use super::Reading;
use super::StateObservation;
use super::Subject;

/// A homogeneous observation payload.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[non_exhaustive]
pub enum Payload {
    Readings(#[cfg_attr(feature = "serde", serde(with = "readings_table"))] Box<[Reading]>),
    Logs(Box<[LogRecord]>),
    States(Box<[StateObservation]>),
    Inventory(Box<[InventoryItem]>),
}

impl Payload {
    pub fn len(&self) -> usize {
        match self {
            Self::Readings(rows) => rows.len(),
            Self::Logs(rows) => rows.len(),
            Self::States(rows) => rows.len(),
            Self::Inventory(rows) => rows.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Canonical immutable unit of telemetry flow.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ObservationBatch {
    pub endpoint: Arc<EndpointContext>,
    pub origin: Origin,
    pub window: ObservationWindow,
    // Private because `new` validates these two against each other.
    coverage: Coverage,
    payload: Payload,
}

impl ObservationBatch {
    /// Assembles a batch and checks the payload against the declared coverage.
    ///
    /// # Errors
    ///
    /// Returns [`BatchError::SubjectOutsideScope`] if an observation names a
    /// subject the declared coverage does not cover.
    pub fn new(
        endpoint: impl Into<Arc<EndpointContext>>,
        origin: Origin,
        window: ObservationWindow,
        coverage: Coverage,
        payload: Payload,
    ) -> Result<Self, BatchError> {
        if let ObservationScope::Subject(expected) = &coverage.scope {
            validate_subject_scope(expected, &payload)?;
        }

        Ok(Self {
            endpoint: endpoint.into(),
            origin,
            window,
            coverage,
            payload,
        })
    }

    pub fn coverage(&self) -> &Coverage {
        &self.coverage
    }

    pub fn payload(&self) -> &Payload {
        &self.payload
    }
}

/// Shared immutable batch used for fan-out.
pub type SharedBatch = Arc<ObservationBatch>;

/// Checks every row that names a subject against the declared scope.
///
/// A log or state without a subject is an endpoint-level observation and is
/// admissible under any scope.
fn validate_subject_scope(expected: &Subject, payload: &Payload) -> Result<(), BatchError> {
    let actual = match payload {
        Payload::Readings(rows) => rows
            .iter()
            .map(|row| &row.signal.subject)
            .find(|subject| *subject != expected),
        Payload::Logs(rows) => rows
            .iter()
            .filter_map(|row| row.subject.as_ref())
            .find(|subject| *subject != expected),
        Payload::States(rows) => rows
            .iter()
            .filter_map(|row| row.subject.as_ref())
            .find(|subject| *subject != expected),
        Payload::Inventory(rows) => rows
            .iter()
            .map(|row| &row.subject)
            .find(|subject| *subject != expected),
    };

    match actual {
        Some(actual) => Err(BatchError::SubjectOutsideScope {
            expected: expected.clone(),
            actual: actual.clone(),
        }),
        None => Ok(()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BatchError {
    SubjectOutsideScope { expected: Subject, actual: Subject },
}

impl fmt::Display for BatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SubjectOutsideScope { expected, actual } => write!(
                formatter,
                "subject '{}:{}' is outside declared scope '{}:{}'",
                actual.kind, actual.id, expected.kind, expected.id
            ),
        }
    }
}

impl Error for BatchError {}

macro_rules! collection_builder {
    ($builder:ident, $row:ty, $what:literal) => {
        #[doc = concat!("Accumulates ", $what, " for a [`Payload`].")]
        ///
        /// Checks nothing itself: a payload is validated against the declared
        /// coverage by [`ObservationBatch::new`].
        #[derive(Debug, Default)]
        pub struct $builder {
            rows: Vec<$row>,
        }

        impl $builder {
            pub fn new() -> Self {
                Self::default()
            }

            pub fn with_capacity(capacity: usize) -> Self {
                Self {
                    rows: Vec::with_capacity(capacity),
                }
            }

            /// Appends a row, preserving source order.
            pub fn push(&mut self, row: $row) {
                self.rows.push(row);
            }

            pub fn len(&self) -> usize {
                self.rows.len()
            }

            pub fn is_empty(&self) -> bool {
                self.rows.is_empty()
            }

            pub fn finish(self) -> Box<[$row]> {
                self.rows.into_boxed_slice()
            }
        }
    };
}

collection_builder!(ReadingsBuilder, Reading, "numeric readings");
collection_builder!(LogsBuilder, LogRecord, "log records");
collection_builder!(StatesBuilder, StateObservation, "state observations");
collection_builder!(InventoryBuilder, InventoryItem, "inventory items");

/// Wire form for [`Payload::Readings`], hoisting descriptors into a table.
///
/// Serde does not carry `Arc` identity, so rows index into a table of distinct
/// descriptors. Entries match by value, so equal payloads encode identically
/// and decoding can only widen sharing. A [`Reading`] serialized alone carries
/// its descriptor inline.
#[cfg(feature = "serde")]
mod readings_table {
    use std::collections::HashMap;
    use std::sync::Arc;

    use serde::Deserialize as _;
    use serde::Serialize as _;

    use crate::model::Attributes;
    use crate::model::Name;
    use crate::model::NumericValue;
    use crate::model::Reading;
    use crate::model::ReportedState;
    use crate::model::SignalDescriptor;
    use crate::model::Timestamp;

    #[derive(serde::Serialize)]
    struct RowRef<'a> {
        source_key: &'a Name,
        signal: usize,
        value: &'a NumericValue,
        observed_at: &'a Option<Timestamp>,
        attributes: &'a Attributes,
        reported_state: &'a Option<ReportedState>,
    }

    #[derive(serde::Serialize)]
    struct TableRef<'a> {
        signals: Vec<&'a SignalDescriptor>,
        rows: Vec<RowRef<'a>>,
    }

    #[derive(serde::Deserialize)]
    struct Row {
        source_key: Name,
        signal: usize,
        value: NumericValue,
        observed_at: Option<Timestamp>,
        attributes: Attributes,
        reported_state: Option<ReportedState>,
    }

    #[derive(serde::Deserialize)]
    struct Table {
        signals: Vec<Arc<SignalDescriptor>>,
        rows: Vec<Row>,
    }

    pub(super) fn serialize<S>(readings: &[Reading], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut signals: Vec<&SignalDescriptor> = Vec::new();
        let mut index_of: HashMap<&SignalDescriptor, usize> = HashMap::new();
        let mut rows = Vec::with_capacity(readings.len());

        for reading in readings {
            // Destructured so a new field on `Reading` fails to compile here
            // rather than vanishing from the wire.
            let Reading {
                source_key,
                signal,
                value,
                observed_at,
                attributes,
                reported_state,
            } = reading;

            let next = signals.len();
            let index = *index_of.entry(signal.as_ref()).or_insert(next);
            if index == next {
                signals.push(signal.as_ref());
            }

            rows.push(RowRef {
                source_key,
                signal: index,
                value,
                observed_at,
                attributes,
                reported_state,
            });
        }

        TableRef { signals, rows }.serialize(serializer)
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Box<[Reading]>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let Table { signals, rows } = Table::deserialize(deserializer)?;

        rows.into_iter()
            .map(|row| {
                let signal = signals.get(row.signal).ok_or_else(|| {
                    serde::de::Error::custom(format!(
                        "reading names signal {} but the table holds {}",
                        row.signal,
                        signals.len()
                    ))
                })?;

                Ok(Reading {
                    source_key: row.source_key,
                    signal: Arc::clone(signal),
                    value: row.value,
                    observed_at: row.observed_at,
                    attributes: row.attributes,
                    reported_state: row.reported_state,
                })
            })
            .collect()
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for ObservationBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Representation {
            endpoint: Arc<EndpointContext>,
            origin: Origin,
            window: ObservationWindow,
            coverage: Coverage,
            payload: Payload,
        }

        let value = <Representation as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(
            value.endpoint,
            value.origin,
            value.window,
            value.coverage,
            value.payload,
        )
        .map_err(serde::de::Error::custom)
    }
}
