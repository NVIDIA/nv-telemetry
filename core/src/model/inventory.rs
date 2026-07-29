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

use super::Attributes;
use super::Subject;

/// One resource in an inventory snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct InventoryItem {
    pub subject: Subject,
    /// The containing resource, when the source reports one.
    ///
    /// This is a containment hint on a flat list, not topology. Arbitrary
    /// typed relationships belong in [`ResourceGraph`](super::ResourceGraph),
    /// which inventory deliberately does not require a consumer to assemble.
    pub parent: Option<Subject>,
    pub attributes: Attributes,
}

impl InventoryItem {
    pub fn new(subject: Subject, attributes: Attributes) -> Self {
        Self {
            subject,
            parent: None,
            attributes,
        }
    }

    #[must_use]
    pub fn with_parent(mut self, parent: Subject) -> Self {
        self.parent = Some(parent);
        self
    }
}
