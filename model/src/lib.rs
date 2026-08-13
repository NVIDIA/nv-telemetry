// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Observation data plane.
//!
//! The public names are validated wrappers that own the model's invariants, so
//! anything downstream of validated ingress may assume they hold.
//!
//! The prost structs generated from `nv.telemetry.v1` are deliberately *not*
//! public. They are the decode target — public fields, no invariants — and
//! exposing them would make "no invariant has been checked here" a convention
//! rather than something the compiler enforces. Bytes reach a caller through a
//! validated type or not at all.
//!
//! This crate has no protocol, I/O, async-runtime, dispatcher, or exporter
//! dependencies.

mod canonical;
mod encode;
mod finite;
mod generated;
mod invalid;
mod rules;
mod value;

#[cfg(test)]
mod tests;

pub use finite::Finite;
/// The schema's numeric bounds, one constant per annotation, generated. Public
/// so hand-written projections — the acquisition crates mapping source fields
/// into this model — enforce the schema's number rather than a copy of it,
/// exactly as this crate's own hand-written validators do.
pub use generated::limits;
/// The validated model: one type per contract message, plus the reshaped
/// enums and the batch payload. Everything here upholds the schema's
/// invariants for as long as it exists.
pub use generated::model::*;
pub use invalid::DecodeError;
pub use invalid::Invalid;
pub use invalid::Violation;
/// The protobuf runtime the wire types were generated against.
///
/// Re-exported because a consumer cannot encode or decode without
/// `prost::Message` in scope, and declaring `prost` separately would let a
/// consumer's version drift from the one these types derive against — a
/// mismatch that shows up as a trait the generated types do not implement.
pub use prost;
pub use value::NumericValue;
pub use value::Timestamp;
pub use value::Value;
pub use value::ValueKind;
