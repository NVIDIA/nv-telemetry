// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Name-resolution gate for generated projection code.
//!
//! `manifests::the_shipped_manifests_emit_the_checked_in_tree` proves the
//! current emitter produces byte-for-byte the module this dependency compiles.
//! Making the source crate a dev-dependency then gives that exact output a
//! real rustc consumer: undefined helpers, wrong source access, model API
//! drift, and accidental generic bounds fail
//! `cargo test -p nv-telemetry-codegen`.

use nv_telemetry_redfish::SensorRead;

struct NonCloneTransport;

#[test]
fn the_emitted_redfish_projection_resolves_in_its_consumer_crate() {
    fn accepts_type<T>() {}

    accepts_type::<SensorRead<NonCloneTransport>>();
}
