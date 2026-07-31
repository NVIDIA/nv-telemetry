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

/// Wire types, generated from the schema.
///
/// Public fields, no invariants: these are what bytes decode into. The
/// validated wrappers that own the model's guarantees take the public names,
/// so an explicit `wire::` at a use site means "no invariant has been checked
/// here yet".
pub use generated::wire;
/// The protobuf runtime the wire types were generated against.
///
/// Re-exported because a consumer cannot encode or decode without
/// `prost::Message` in scope, and declaring `prost` separately would let a
/// consumer's version drift from the one these types derive against — a
/// mismatch that shows up as a trait the generated types do not implement.
pub use prost;
