// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Typed projections from `nv-redfish` schema values into `nv-telemetry`.

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
pub use sensor::SensorSampleProjection;
pub use signal::SignalCatalog;
pub use signal::SignalDescriptorRecord;
pub use signal::SignalKey;
pub use signal::SignalSample;
pub use signal::SignalUpdate;
pub use signal::UnresolvedSignal;
