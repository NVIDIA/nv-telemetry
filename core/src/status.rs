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

use std::sync::Arc;

use crate::EndpointContext;
use crate::ObservationWindow;
use crate::Origin;

/// Stable, source-neutral classification of an acquisition failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum FailureClass {
    Transport,
    Timeout,
    Authentication,
    Authorization,
    RateLimited,
    Unsupported,
    InvalidResponse,
    Cancelled,
    Other,
}

/// Whether attempting the same acquisition again could answer differently.
///
/// Named rather than a bare `bool` so a call site states which it means. The
/// wire form stays a boolean.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(from = "bool", into = "bool"))]
pub enum Retryable {
    Yes,
    No,
}

impl From<bool> for Retryable {
    fn from(value: bool) -> Self {
        if value {
            Self::Yes
        } else {
            Self::No
        }
    }
}

impl From<Retryable> for bool {
    fn from(value: Retryable) -> Self {
        matches!(value, Retryable::Yes)
    }
}

/// Operational result of one acquisition attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum AcquisitionOutcome {
    Succeeded {
        emitted_batches: u32,
    },
    Failed {
        class: FailureClass,
        retryable: Retryable,
    },
}

impl AcquisitionOutcome {
    pub const fn succeeded(emitted_batches: u32) -> Self {
        Self::Succeeded { emitted_batches }
    }

    pub const fn failed(class: FailureClass, retryable: Retryable) -> Self {
        Self::Failed { class, retryable }
    }

    /// Returns whether repeating this acquisition could answer differently.
    ///
    /// A success has nothing to retry, so it reports [`Retryable::No`].
    pub const fn retryable(self) -> Retryable {
        match self {
            Self::Succeeded { .. } => Retryable::No,
            Self::Failed { retryable, .. } => retryable,
        }
    }
}

/// Operational status emitted separately from device observations.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
#[non_exhaustive]
pub struct AcquisitionStatus {
    pub endpoint: Arc<EndpointContext>,
    pub origin: Origin,
    pub window: ObservationWindow,
    pub outcome: AcquisitionOutcome,
}

impl AcquisitionStatus {
    pub fn new(
        endpoint: impl Into<Arc<EndpointContext>>,
        origin: Origin,
        window: ObservationWindow,
        outcome: AcquisitionOutcome,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            origin,
            window,
            outcome,
        }
    }
}
