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
use super::Reachability;
use super::Reading;
use super::ResourceGraph;
use super::StateObservation;
use super::Subject;

/// A homogeneous observation payload.
///
/// Row order is part of the identity of readings, logs, states, and inventory.
/// The model does not assume those domains are unordered and does not sort
/// them. A producer using batch hashes for change detection should therefore
/// emit rows in a deterministic source-defined order.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[non_exhaustive]
pub enum Payload {
    Readings(#[cfg_attr(feature = "serde", serde(with = "readings_table"))] Box<[Reading]>),
    Logs(Box<[LogRecord]>),
    States(Box<[StateObservation]>),
    Inventory(Box<[InventoryItem]>),
    Resources(ResourceGraph),
}

impl Payload {
    pub fn len(&self) -> usize {
        match self {
            Self::Readings(rows) => rows.len(),
            Self::Logs(rows) => rows.len(),
            Self::States(rows) => rows.len(),
            Self::Inventory(rows) => rows.len(),
            // A graph counts relations too: consumers use this for size
            // accounting and relations dominate a densely linked graph.
            Self::Resources(graph) => graph.resource_count() + graph.relation_count(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Checks every row that names a subject against the declared scope.
    ///
    /// A log or state without a subject is an endpoint-level observation and
    /// is admissible under any scope.
    fn check_within_scope(&self, expected: &Subject) -> Result<(), BatchError> {
        let stray = match self {
            Self::Readings(rows) => rows
                .iter()
                .map(|row| &row.signal.subject)
                .find(|subject| *subject != expected),
            Self::Logs(rows) => rows
                .iter()
                .filter_map(|row| row.subject.as_ref())
                .find(|subject| *subject != expected),
            Self::States(rows) => rows
                .iter()
                .filter_map(|row| row.subject.as_ref())
                .find(|subject| *subject != expected),
            Self::Inventory(rows) => rows
                .iter()
                .map(|row| &row.subject)
                .find(|subject| *subject != expected),
            Self::Resources(graph) => return check_graph_within_scope(graph, expected),
        };

        match stray {
            Some(actual) => Err(BatchError::SubjectOutsideScope {
                expected: expected.clone(),
                actual: actual.clone(),
            }),
            None => Ok(()),
        }
    }
}

/// Canonical immutable unit of telemetry flow.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "BatchParts"))]
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
    /// subject the declared coverage does not cover. For a resource graph
    /// under a subject scope, returns [`BatchError::MissingScopeRoot`] or
    /// [`BatchError::UnreachableFromScopeRoot`] if the graph is not a subtree
    /// hanging off that root.
    pub fn new(
        endpoint: impl Into<Arc<EndpointContext>>,
        origin: Origin,
        window: ObservationWindow,
        coverage: Coverage,
        payload: Payload,
    ) -> Result<Self, BatchError> {
        if let ObservationScope::Subject(expected) = &coverage.scope {
            payload.check_within_scope(expected)?;
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

/// Checks that a graph is exactly the subtree rooted at the scope subject.
///
/// Row payloads are scoped by requiring every row to carry the scope subject,
/// but a graph is a connected structure rather than a list of independent
/// rows. The natural unit of partial graph collection is a subtree, such as one
/// chassis and everything it contains, so scope here means reachability from
/// the root rather than subject equality.
fn check_graph_within_scope(graph: &ResourceGraph, root: &Subject) -> Result<(), BatchError> {
    // Checked before the root, which an empty graph cannot hold. Observing
    // nothing contradicts no scope, exactly as an empty row payload does not.
    if graph.is_empty() {
        return Ok(());
    }

    match graph.reachability_from(root) {
        Reachability::MissingRoot => Err(BatchError::MissingScopeRoot(root.clone())),
        Reachability::Unreachable(subject) => Err(BatchError::UnreachableFromScopeRoot {
            root: root.clone(),
            subject: subject.clone(),
        }),
        Reachability::FullyReachable => Ok(()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BatchError {
    SubjectOutsideScope { expected: Subject, actual: Subject },
    MissingScopeRoot(Subject),
    UnreachableFromScopeRoot { root: Subject, subject: Subject },
}

impl fmt::Display for BatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SubjectOutsideScope { expected, actual } => write!(
                formatter,
                "subject '{}:{}' is outside declared scope '{}:{}'",
                actual.kind, actual.id, expected.kind, expected.id
            ),
            Self::MissingScopeRoot(root) => write!(
                formatter,
                "scope root '{}:{}' is not present in the resource graph",
                root.kind, root.id
            ),
            Self::UnreachableFromScopeRoot { root, subject } => write!(
                formatter,
                "resource '{}:{}' is not reachable from scope root '{}:{}'",
                subject.kind, subject.id, root.kind, root.id
            ),
        }
    }
}

impl Error for BatchError {}

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
    use crate::model::NumericValue;
    use crate::model::Reading;
    use crate::model::ReportedState;
    use crate::model::SignalDescriptor;
    use crate::model::SourceKey;
    use crate::model::Timestamp;

    #[derive(serde::Serialize)]
    struct RowRef<'a> {
        source_key: &'a SourceKey,
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
    #[serde(deny_unknown_fields)]
    struct Row {
        source_key: SourceKey,
        signal: usize,
        value: NumericValue,
        observed_at: Option<Timestamp>,
        attributes: Attributes,
        reported_state: Option<ReportedState>,
    }

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Table {
        signals: Vec<Arc<SignalDescriptor>>,
        rows: Vec<Row>,
    }

    // The descriptors carry a cached digest, which counts as interior
    // mutability even though it is derived from the content the key compares
    // by and cannot change what the key hashes to.
    #[allow(clippy::mutable_key_type)]
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

/// The unvalidated fields an [`ObservationBatch`] is assembled from.
///
/// Every field of the batch must appear here. `deny_unknown_fields` is what
/// keeps the two in step: a field added there and not here is written by the
/// derive and then refused on the way back, by name in a named format and by
/// length in a positional one, rather than decoding as absent.
#[cfg(feature = "serde")]
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchParts {
    endpoint: Arc<EndpointContext>,
    origin: Origin,
    window: ObservationWindow,
    coverage: Coverage,
    payload: Payload,
}

#[cfg(feature = "serde")]
impl TryFrom<BatchParts> for ObservationBatch {
    type Error = BatchError;

    fn try_from(value: BatchParts) -> Result<Self, Self::Error> {
        Self::new(
            value.endpoint,
            value.origin,
            value.window,
            value.coverage,
            value.payload,
        )
    }
}
