// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Planning and dispatcher vocabulary.
//!
//! Resolves data needs into provider selections and dispatcher subtree
//! recipes. Capability probes are emitted as dispatched work rather than
//! issued directly, so planning is asynchronous and a resolved plan is allowed
//! to be partial.
