// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Emitter behavior on schema shapes the shipped contract does not exercise.
//!
//! The byte-compare against the checked-in model proves the emitter's output
//! for the contract as it is; these fixtures cover the shapes it could grow —
//! where "works on the current contract" and "works" quietly diverge.

use std::path::Path;
use std::path::PathBuf;

use nv_telemetry_codegen::options::Vocabulary;
use nv_telemetry_codegen::wrapper;
use prost_reflect::DescriptorPool;

/// Compiles a throwaway contract-package schema alongside the real vocabulary.
fn pool_from(tag: &str, schema: &str) -> DescriptorPool {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("{}-emitter-{tag}", std::process::id()));
    let dir = root.join("nv/telemetry/v1");
    std::fs::create_dir_all(&dir).expect("fixture directory is writable");
    std::fs::write(dir.join("probe.proto"), schema).expect("fixture is writable");

    let roots = vec![root, manifest.join("../schema/proto")];
    let mut compiler = protox::Compiler::new(&roots).expect("include paths resolve");
    compiler.include_imports(true);
    compiler.include_source_info(false);
    compiler
        .open_files([Path::new("nv/telemetry/v1/probe.proto")])
        .expect("fixture schema compiles");
    DescriptorPool::decode(compiler.encode_file_descriptor_set().as_slice())
        .expect("fixture descriptors load")
}

const PREFIX: &str = "syntax = \"proto3\";\n\
    package nv.telemetry.v1;\n\
    import \"nv/telemetry/options/v1/annotations.proto\";\n";

#[test]
fn an_enum_value_colliding_with_a_derived_variant_is_refused() {
    // Variant names are derived, and the generator claims `Unrecognized` for
    // itself. A duplicate variant is still parseable Rust, so without the
    // check this fails in the model crate's build, blamed on a checked-in
    // file instead of the schema name that caused it.
    let pool = pool_from(
        "collision",
        &format!(
            "{PREFIX}
enum Probe {{
  PROBE_UNSPECIFIED = 0;
  PROBE_UNRECOGNIZED = 1;
}}
message Holder {{
  optional Probe p = 1 [(nv.telemetry.options.v1.field_invariant) = {{reject_unspecified: true}}];
}}
"
        ),
    );
    let vocabulary = Vocabulary::resolve(&pool).expect("vocabulary resolves");

    let error = wrapper::model(&pool, &vocabulary).expect_err("the collision is refused");
    assert!(
        error.contains("PROBE_UNRECOGNIZED") && error.contains("Unrecognized"),
        "the error does not name the colliding value: {error}"
    );
}

#[test]
fn every_declared_oneof_is_planned() {
    // The contract has one oneof, so a planner that took only the first would
    // pass every byte-compare while silently dropping the second message's
    // worth of fields the day a second oneof lands.
    let pool = pool_from(
        "oneofs",
        &format!(
            "{PREFIX}
message Inner {{}}
message Holder {{
  oneof first {{
    Inner a = 1;
  }}
  oneof second {{
    Inner b = 2;
  }}
}}
"
        ),
    );
    let vocabulary = Vocabulary::resolve(&pool).expect("vocabulary resolves");

    let rendered = wrapper::model(&pool, &vocabulary).expect("two oneofs generate");
    assert!(rendered.contains("pub enum First"), "first oneof missing");
    assert!(
        rendered.contains("pub enum Second"),
        "the second oneof was silently dropped from planning"
    );
}

#[test]
fn an_enum_value_deriving_an_unusable_variant_is_refused() {
    // `FORM_FACTOR_2U` is realistic hardware vocabulary, and stripping the
    // prefix leaves a variant name starting with a digit.
    let pool = pool_from(
        "digit",
        &format!(
            "{PREFIX}
enum FormFactor {{
  FORM_FACTOR_UNSPECIFIED = 0;
  FORM_FACTOR_2U = 1;
}}
message Holder {{
  optional FormFactor f = 1 [(nv.telemetry.options.v1.field_invariant) = {{reject_unspecified: true}}];
}}
"
        ),
    );
    let vocabulary = Vocabulary::resolve(&pool).expect("vocabulary resolves");

    let error = wrapper::model(&pool, &vocabulary).expect_err("the digit variant is refused");
    assert!(
        error.contains("FORM_FACTOR_2U"),
        "the error does not name the value: {error}"
    );
}

#[test]
fn a_message_whose_rules_name_is_a_keyword_is_refused() {
    // `Match` is a legal, styled message name; its rules-registry function
    // would be `fn match`, which is not a function anyone can write.
    let pool = pool_from("keyword", &format!("{PREFIX}\nmessage Match {{}}\n"));
    let vocabulary = Vocabulary::resolve(&pool).expect("vocabulary resolves");

    let error =
        wrapper::model(&pool, &vocabulary).expect_err("the keyword-derived name is refused");
    assert!(
        error.contains("Match"),
        "the error does not name the message: {error}"
    );
}

#[test]
fn a_derived_name_colliding_with_the_models_vocabulary_is_refused() {
    // A oneof named `value` derives `pub enum Value` — colliding with the
    // hand-written `Value` the generated module imports. Duplicate names
    // parse, so without the check this is a downstream build error blamed on
    // a checked-in file.
    let pool = pool_from(
        "shadow",
        &format!(
            "{PREFIX}
message Inner {{}}
message Holder {{
  oneof value {{
    Inner a = 1;
  }}
}}
"
        ),
    );
    let vocabulary = Vocabulary::resolve(&pool).expect("vocabulary resolves");

    let error = wrapper::model(&pool, &vocabulary).expect_err("the shadowing name is refused");
    assert!(
        error.contains("Value"),
        "the error does not name the collision: {error}"
    );
}
