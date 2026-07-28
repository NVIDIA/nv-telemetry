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

//! Source-neutral telemetry observation types.
//!
//! This crate contains only the immutable data plane. It deliberately has no
//! protocol, I/O, async-runtime, dispatcher, exporter, or health-policy
//! dependencies.
//!
//! A batch contains exactly one payload domain:
//!
//! ```
//! use std::sync::Arc;
//! use nv_telemetry_core::{
//!     Attributes, Coverage, EndpointContext, ObservationBatch, ObservationWindow,
//!     Origin, Payload, Timestamp,
//! };
//!
//! let endpoint = Arc::new(EndpointContext::new("bmc-1", Attributes::empty()));
//! let observed_at = Timestamp::new(1_700_000_000, 0)?;
//! let window = ObservationWindow::point(observed_at);
//! let payloads = [
//!     Payload::Readings(Box::new([])),
//!     Payload::Logs(Box::new([])),
//!     Payload::States(Box::new([])),
//!     Payload::Inventory(Box::new([])),
//! ];
//!
//! for payload in payloads {
//!     let batch = ObservationBatch::new(
//!         Arc::clone(&endpoint),
//!         Origin::new("example-provider", "example-request"),
//!         window,
//!         Coverage::complete_endpoint(),
//!         payload,
//!     )?;
//!     assert!(batch.payload().is_empty());
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

// Compiles the README's examples so they cannot drift from the API.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct Readme;

pub mod model;
pub mod status;

pub use model::AttrKey;
pub use model::AttrValue;
pub use model::Attribute;
pub use model::Attributes;
pub use model::AttributesError;
pub use model::BatchError;
pub use model::Completeness;
pub use model::Coverage;
pub use model::EndpointContext;
pub use model::EndpointId;
pub use model::Finite;
pub use model::InventoryBuilder;
pub use model::InventoryItem;
pub use model::LogRecord;
pub use model::LogsBuilder;
pub use model::Name;
pub use model::NonFiniteError;
pub use model::NumericValue;
pub use model::ObservationBatch;
pub use model::ObservationScope;
pub use model::ObservationWindow;
pub use model::Origin;
pub use model::Payload;
pub use model::RangeOrderError;
pub use model::Reading;
pub use model::ReadingBounds;
pub use model::ReadingKind;
pub use model::ReadingsBuilder;
pub use model::ReportedState;
pub use model::Severity;
pub use model::SharedBatch;
pub use model::SignalDescriptor;
pub use model::StateObservation;
pub use model::StatesBuilder;
pub use model::Subject;
pub use model::Thresholds;
pub use model::Timestamp;
pub use model::TimestampError;
pub use model::Unit;
pub use status::AcquisitionOutcome;
pub use status::AcquisitionStatus;
pub use status::FailureClass;
