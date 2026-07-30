// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Observation data plane.
//!
//! Wire types are generated from the `nv.telemetry.v1` schema; the public
//! names are validated wrappers that own the model's invariants, so anything
//! downstream of validated ingress may assume they hold.
//!
//! This crate has no protocol, I/O, async-runtime, dispatcher, or exporter
//! dependencies.

mod generated;
