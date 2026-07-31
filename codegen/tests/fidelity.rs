// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Proves the generated wire types and the shipped descriptors agree.
//!
//! Structural spot-checking — "does every message exist, is every optional an
//! Option" — cannot catch a field number that shifted or a type that was
//! rendered as the wrong width, because both sides would still look
//! plausible. Encoding with one and decoding with the other does catch it:
//! the two representations have to agree byte for byte or the round trip
//! fails.
//!
//! This matters because the generated types are checked in. A consumer holds
//! them, a source builds them, and nothing else in the pipeline re-derives
//! them from the schema at runtime.

use nv_telemetry_model::wire;
use prost::Message as _;
use prost_reflect::DynamicMessage;
use prost_reflect::Value as V;

#[test]
fn generated_types_and_descriptors_encode_identically() {
    let pool = nv_telemetry_codegen::pool().expect("shipped schema decodes");

    // Built with the generated types.
    let native = wire::Subject {
        kind: Some("sensor".into()),
        scope: vec!["1U".into(), "PSU1".into()],
        id: Some("CPU1Temp".into()),
    };

    // The same value, built against the descriptors the rules ran over.
    let descriptor = pool
        .get_message_by_name("nv.telemetry.v1.Subject")
        .expect("Subject is in the shipped schema");
    let mut dynamic = DynamicMessage::new(descriptor.clone());
    dynamic.set_field_by_name("kind", V::String("sensor".into()));
    dynamic.set_field_by_name(
        "scope",
        V::List(vec![V::String("1U".into()), V::String("PSU1".into())]),
    );
    dynamic.set_field_by_name("id", V::String("CPU1Temp".into()));

    assert_eq!(
        native.encode_to_vec(),
        dynamic.encode_to_vec(),
        "the generated type and the descriptors disagree about the wire format"
    );

    // And across the boundary in both directions.
    let from_dynamic = wire::Subject::decode(dynamic.encode_to_vec().as_slice())
        .expect("descriptor-encoded bytes decode into the generated type");
    assert_eq!(from_dynamic, native);

    DynamicMessage::decode(descriptor, native.encode_to_vec().as_slice())
        .expect("generated-encoded bytes decode against the descriptors");
}

#[test]
fn oneof_arms_carry_the_field_numbers_the_schema_declares() {
    let pool = nv_telemetry_codegen::pool().expect("shipped schema decodes");
    let descriptor = pool
        .get_message_by_name("nv.telemetry.v1.NumericValue")
        .expect("NumericValue is in the shipped schema");

    // Arm selection is a wire-level fact: picking the wrong tag would silently
    // reinterpret an integer counter as a float.
    for (name, native, value) in [
        (
            "uint_value",
            wire::NumericValue {
                kind: Some(wire::numeric_value::Kind::UintValue(91_827_364_554_433_777)),
            },
            V::U64(91_827_364_554_433_777),
        ),
        (
            "int_value",
            wire::NumericValue {
                kind: Some(wire::numeric_value::Kind::IntValue(-42)),
            },
            V::I64(-42),
        ),
    ] {
        let mut dynamic = DynamicMessage::new(descriptor.clone());
        dynamic.set_field_by_name(name, value);
        assert_eq!(
            native.encode_to_vec(),
            dynamic.encode_to_vec(),
            "generated `{name}` does not match the descriptor's encoding"
        );
    }
}

#[test]
fn the_checked_in_wire_types_match_the_schema() {
    // `make check-codegen` covers this in CI, but only there. A hand-edit to
    // the generated file — a shifted tag, a hand-"fixed" type — otherwise
    // survives a full `cargo test` run, and the generated types are checked
    // in precisely so that people read and touch them.
    let pool = nv_telemetry_codegen::pool().expect("shipped schema decodes");
    let rendered = nv_telemetry_codegen::wire::generate(&pool).expect("wire types render");

    let committed = std::fs::read_to_string(
        nv_telemetry_codegen::workspace_root()
            .expect("run from the repo")
            .join("model/src/generated/wire.rs"),
    )
    .expect("the generated wire types are committed");

    assert_eq!(committed, rendered, "run `make codegen`");
}

#[test]
fn a_contract_sub_package_is_refused_rather_than_dropped() {
    // The file filter matches sub-packages but only one module is emitted, so
    // a sub-package would be linted, locked, and compatibility-checked while
    // having no type in the data plane — and every gate would stay green.
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("{}-subpkg", std::process::id()));
    let dir = root.join("nv/telemetry/v1/gpu");
    std::fs::create_dir_all(&dir).expect("fixture directory is writable");
    std::fs::write(
        dir.join("gpu.proto"),
        "syntax = \"proto3\";\npackage nv.telemetry.v1.gpu;\nmessage GpuThing { optional string id = 1; }\n",
    )
    .expect("fixture is writable");

    let mut compiler = protox::Compiler::new([&root, &manifest.join("../schema/proto")])
        .expect("include paths resolve");
    compiler.include_imports(true);
    // Both the sub-package and a real contract file, so the generator sees the
    // shape it would see in the repo: two modules where it emits one.
    compiler
        .open_files([
            std::path::Path::new("nv/telemetry/v1/gpu/gpu.proto"),
            std::path::Path::new("nv/telemetry/v1/subject.proto"),
        ])
        .expect("fixture compiles");
    let pool =
        prost_reflect::DescriptorPool::decode(compiler.encode_file_descriptor_set().as_slice())
            .expect("fixture descriptors load");

    let error = nv_telemetry_codegen::wire::generate(&pool)
        .expect_err("a sub-package must be refused, not silently dropped");
    assert!(error.contains("sub-package"), "unexpected error: {error}");
}
