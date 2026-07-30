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

//! Fixtures shared by the core model test binaries.
//!
//! Every item here is used by both binaries, so the module needs no dead-code
//! allowance; anything added for one of them alone should carry its own
//! `#[allow(dead_code)]` rather than a blanket allowance on the module.

use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::Arc;

use nv_telemetry_core::Attribute;
use nv_telemetry_core::Attributes;
use nv_telemetry_core::EndpointContext;
use nv_telemetry_core::Timestamp;

pub(crate) fn timestamp(seconds: i64) -> Timestamp {
    Timestamp::new(seconds, 0).expect("valid timestamp")
}

/// The endpoint the fixtures are taken to have been collected from.
pub(crate) fn endpoint() -> Arc<EndpointContext> {
    Arc::new(EndpointContext::new(
        "bmc-00:11:22:33:44:55",
        Attributes::new(vec![
            Attribute::new("device_class", "compute_node"),
            Attribute::new("rack", "rack-1"),
        ])
        .expect("unique attributes"),
    ))
}

/// Digests one value the way a caller content-addressing an observation does.
///
/// The hasher is fixed here so two binaries cannot disagree about what a
/// digest is while both claiming to check the same property.
pub(crate) fn hash_of<T: Hash>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}
