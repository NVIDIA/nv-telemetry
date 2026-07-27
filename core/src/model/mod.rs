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
mod context;
mod name;
mod number;
mod time;

pub use attributes::{AttrKey, AttrValue, Attribute, Attributes, AttributesError};
pub use context::{
    Completeness, Coverage, EndpointContext, EndpointId, ObservationScope, Origin, Subject,
};
pub use name::Name;
pub use number::{Finite, NonFiniteError};
pub use time::{ObservationWindow, Timestamp, TimestampError};
