// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Provenance tables.
//!
//! Emits the mapping from each contract field to the source path it was
//! projected from, so tooling can answer where a reading came from. This is
//! the planner's explainability requirement extended to data.
