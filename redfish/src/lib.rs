// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Typed, I/O-free projections from `nv-redfish` schema values into
//! [`nv_telemetry_core`] observations.
//!
//! Sensor metadata, samples, and resource state are projected separately.
//! [`SignalCatalog`] joins samples to endpoint-local metadata, while
//! [`SensorResourceRecord`] keeps a projected resource with the parent
//! relation needed for graph reachability.
//!
//! [`Project`] is a static interface. Runtime dispatch should store typed
//! function pointers such as `P::project` or use an application-owned adapter.
//!
//! See the [crate README](https://github.com/NVIDIA/nv-telemetry/tree/main/redfish)
//! for compiled examples and the complete identity and threshold contracts.

// Compiles the README's Rust examples so they cannot drift from the API.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct Readme;

mod projection;
mod sensor;
mod signal;
mod uri;

pub use projection::FieldValue;
pub use projection::Fields;
pub use projection::Project;
pub use projection::ProjectionIssue;
pub use projection::ProjectionIssueKind;
pub use projection::ProjectionResult;
pub use sensor::SensorMetadataProjection;
pub use sensor::SensorProjectionContext;
pub use sensor::SensorResourceProjection;
pub use sensor::SensorResourceRecord;
pub use sensor::SensorSampleProjection;
pub use signal::SignalCatalog;
pub use signal::SignalCatalogError;
pub use signal::SignalCatalogFull;
pub use signal::SignalDescriptorRecord;
pub use signal::SignalKey;
pub use signal::SignalRevisionExhausted;
pub use signal::SignalSample;
pub use signal::SignalUpdate;
pub use signal::UnresolvedSignal;
