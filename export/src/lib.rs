// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Shared exporter machinery.
//!
//! Holds what exporters have in common: joining readings to their descriptors
//! by signal key, flattening subjects into labels or attributes, and unit
//! handling. Format-specific mapping lives in the individual exporter crates,
//! whose dependency trees are disjoint.
