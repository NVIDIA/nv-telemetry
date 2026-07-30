// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Acquisition contract shared by every protocol.
//!
//! Defines what a source implements: source and stage traits, capability
//! probes, endpoint access, and request classification. The planner plans
//! against this contract and protocol crates implement it, so neither depends
//! on the other.
