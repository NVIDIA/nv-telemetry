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

// Unlike the shipped Sensor manifest, this checked fixture exercises both
// Rust shapes that need explicit absence/null/present state. Its golden test
// in `manifests.rs` proves these are the current emitter's bytes; including it
// here makes rustc resolve every generated source access and ownership move.
mod uri {
    pub(crate) fn canonical(uri: &str) -> &str {
        let path = uri.split_once('#').map_or(uri, |(path, _)| path);
        let path = path.split_once('?').map_or(path, |(path, _)| path);
        let trimmed = path.trim_end_matches('/');
        if trimmed.is_empty() && path.starts_with('/') {
            "/"
        } else {
            trimmed
        }
    }
}

#[allow(dead_code)]
#[rustfmt::skip]
#[path = "fixtures/projection_presence.rs"]
mod projection_presence;

struct NonCloneTransport;

#[test]
fn the_emitted_redfish_projection_resolves_in_its_consumer_crate() {
    fn accepts_type<T>() {}

    accepts_type::<SensorRead<NonCloneTransport>>();

    let _ = projection_presence::project_boot_option;
    let _ = projection_presence::project_chassis;
}
