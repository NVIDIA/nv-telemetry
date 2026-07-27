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
//! Every observation is attributed to an endpoint, a subject, and the provider
//! that produced it, and is qualified by how much of a declared scope it
//! covers:
//!
//! ```
//! use nv_telemetry_core::{
//!     Attributes, Completeness, Coverage, EndpointContext, ObservationScope,
//!     ObservationWindow, Origin, Subject, Timestamp,
//! };
//!
//! let endpoint = EndpointContext::new("bmc-1", Attributes::empty());
//! let observed_at = Timestamp::new(1_700_000_000, 0)?;
//! let window = ObservationWindow::point(observed_at);
//! let origin = Origin::new("example-provider", "example-request");
//!
//! // A partial sweep of one sensor cannot be used to infer that anything else
//! // is absent, so the claim it carries is narrowed on both axes.
//! let coverage = Coverage::new(
//!     ObservationScope::Subject(Subject::new("sensor", "CPU0Temp")),
//!     Completeness::partial(Some(1)),
//! );
//!
//! assert_eq!(endpoint.id.as_str(), "bmc-1");
//! assert_eq!(window.started_at(), observed_at);
//! assert_eq!(origin.provider.as_str(), "example-provider");
//! assert!(!coverage.completeness.is_complete());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod model;
pub mod status;

pub use model::AttrKey;
pub use model::AttrValue;
pub use model::Attribute;
pub use model::Attributes;
pub use model::AttributesError;
pub use model::Completeness;
pub use model::Coverage;
pub use model::EndpointContext;
pub use model::EndpointId;
pub use model::Finite;
pub use model::Name;
pub use model::NonFiniteError;
pub use model::ObservationScope;
pub use model::ObservationWindow;
pub use model::Origin;
pub use model::Subject;
pub use model::Timestamp;
pub use model::TimestampError;
pub use status::AcquisitionOutcome;
pub use status::AcquisitionStatus;
pub use status::FailureClass;
