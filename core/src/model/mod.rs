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

//! Immutable observation model.
//!
//! Completeness is relative to a batch's declared [`ObservationScope`]. A
//! complete snapshot may establish absence only inside that scope. A partial
//! batch cannot establish absence, a failed acquisition produces no batch, and
//! staleness remains a consumer policy based on observation timestamps.

mod attributes;
mod batch;
mod collection;
mod context;
mod inventory;
mod log;
mod name;
mod number;
mod property;
mod reading;
mod resource;
mod state;
mod time;

pub use attributes::AttrKey;
pub use attributes::AttrValue;
pub use attributes::Attribute;
pub use attributes::Attributes;
pub use attributes::AttributesError;
pub use batch::BatchError;
pub use batch::ObservationBatch;
pub use batch::Payload;
pub use batch::SharedBatch;
pub use context::Completeness;
pub use context::Coverage;
pub use context::EndpointContext;
pub use context::EndpointId;
pub use context::ObservationScope;
pub use context::Origin;
pub use context::Provider;
pub use context::RequestClass;
pub use context::SourceKey;
pub use context::Subject;
pub use context::SubjectId;
pub use context::SubjectKind;
pub use inventory::InventoryItem;
pub use log::LogRecord;
pub use log::Severity;
pub use name::Name;
pub use number::Finite;
pub use number::NonFiniteError;
pub use property::Property;
pub use property::PropertyArray;
pub use property::PropertyArrayError;
pub use property::PropertyMap;
pub use property::PropertyMapError;
pub use property::PropertyValue;
pub use property::ResourceReference;
pub use reading::Health;
pub use reading::Instance;
pub use reading::Metric;
pub use reading::NumericValue;
pub use reading::OperatingState;
pub use reading::RangeOrderError;
pub use reading::Reading;
pub use reading::ReadingKind;
pub use reading::ReportedState;
pub use reading::SignalDescriptor;
pub use reading::Unit;
pub use reading::ValueRange;
pub use resource::GraphLimits;
pub use resource::ObservedResource;
pub use resource::Reachability;
pub use resource::RelationKind;
pub use resource::ResourceCompleteness;
pub use resource::ResourceGraph;
pub use resource::ResourceGraphError;
pub use resource::ResourceRelation;
pub use state::StateObservation;
pub use time::DurationValue;
pub use time::ObservationWindow;
pub use time::TimeError;
pub use time::Timestamp;
