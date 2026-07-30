// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Covers the annotation reader and the presence lint together.
//!
//! Reading a custom option is the compiler's foundation, and getting it wrong
//! is quiet: an option that fails to resolve reads as its default, so a field
//! that exempts itself from a rule would silently stop being exempt. The
//! fixtures therefore plant known violations among fields that are exempt for
//! several different reasons, so a broken reader shows up as a wrong set of
//! violations rather than as nothing at all.

use std::path::Path;
use std::path::PathBuf;

use nv_telemetry_codegen::lint;
use nv_telemetry_codegen::options::Vocabulary;
use nv_telemetry_codegen::options::VocabularyErrorKind;
use prost_reflect::DescriptorPool;

fn pool_for(file: &str) -> DescriptorPool {
    pool_with_vocabulary(file, None)
}

/// Compiles `file`, optionally shadowing the real annotation vocabulary with a
/// deliberately wrong one from `vocabulary_root`. Shadowing works because the
/// first include path that resolves a name wins, which lets a fixture exercise
/// a broken vocabulary without a second copy of the contract.
fn pool_with_vocabulary(file: &str, vocabulary_root: Option<&str>) -> DescriptorPool {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixtures = manifest.join("tests/fixtures");
    let contract = manifest.join("../schema/proto");

    let mut roots = Vec::new();
    if let Some(root) = vocabulary_root {
        roots.push(fixtures.join(root));
    }
    roots.push(fixtures.clone());
    roots.push(contract);

    let mut compiler = protox::Compiler::new(&roots).expect("include paths resolve");
    compiler.include_imports(true);
    compiler.include_source_info(false);
    compiler
        .open_files([Path::new(file)])
        .expect("fixture schema compiles");

    // Encoded through reflection rather than through
    // prost_types::FileDescriptorSet, which has nowhere to store extensions
    // and would drop every annotation these tests are about. The schema
    // crate's build script takes the same path for the same reason.
    DescriptorPool::decode(compiler.encode_file_descriptor_set().as_slice())
        .expect("fixture descriptors load")
}

fn fixture(file: &str) -> (DescriptorPool, Vocabulary) {
    let pool = pool_for(file);
    let vocabulary = Vocabulary::resolve(&pool).expect("fixture defines the vocabulary");
    (pool, vocabulary)
}

#[test]
fn presence_lint_catches_only_the_unexcused_scalar() {
    let (pool, vocabulary) = fixture("presence.proto");

    let violations = lint::presence(&pool, &vocabulary);
    let fields: Vec<_> = violations.iter().map(lint::Violation::subject).collect();

    assert_eq!(fields, ["nv.telemetry.v1.PresenceFixture.implicit"]);
}

#[test]
fn presence_lint_reaches_nested_types_and_skips_what_it_cannot_fix() {
    let (pool, vocabulary) = fixture("edges.proto");

    let violations = lint::presence(&pool, &vocabulary);
    let fields: Vec<_> = violations.iter().map(lint::Violation::subject).collect();

    // Nested declarations are reachable, so they are linted. A scalar-valued
    // map is caught on the map field rather than on its synthetic entry: an
    // entry that carries only its key decodes the value as zero, and the entry
    // itself can be neither annotated nor made optional. A message-valued map
    // keeps absence expressible and passes. Oneof members and repeated fields
    // already distinguish absent from empty.
    assert_eq!(
        fields,
        [
            "nv.telemetry.v1.Edges.Nested.nested_implicit",
            "nv.telemetry.v1.Edges.blob",
            "nv.telemetry.v1.Edges.labels",
            "nv.telemetry.v1.Edges.misannotated",
            "nv.telemetry.v1.Edges.severity",
            "nv.telemetry.v1.Edges.wrapped",
            "nv.telemetry.v1.Edges.wrapped_by_name",
        ]
    );
}

#[test]
fn rules_other_than_presence_are_reachable() {
    let (pool, vocabulary) = fixture("rules.proto");
    let found: Vec<_> = lint::presence(&pool, &vocabulary)
        .into_iter()
        .map(|violation| (violation.subject().to_owned(), violation.reason().clone()))
        .collect();

    // Asserting the reason, not just the subject: two rules can fire on the
    // same declaration shape, so a test that checked names alone would pass
    // with the two swapped.
    assert_eq!(
        found,
        [
            (
                "nv.telemetry.v1.Hashed".to_owned(),
                lint::Reason::HashableWithoutValidated
            ),
            (
                "nv.telemetry.v1.sneaky".to_owned(),
                lint::Reason::ContractExtension
            ),
        ]
    );
}

#[test]
fn a_proto2_contract_file_is_rejected() {
    let (pool, vocabulary) = fixture("legacy.proto");
    let reasons: Vec<_> = lint::presence(&pool, &vocabulary)
        .into_iter()
        .map(|violation| violation.reason().clone())
        .collect();

    assert!(reasons.contains(&lint::Reason::UnsupportedSyntax));
}

#[test]
fn a_vocabulary_missing_a_field_is_an_error() {
    let pool = pool_with_vocabulary("minimal.proto", Some("badvocab"));
    let error = Vocabulary::resolve(&pool).expect_err("the vocabulary is missing `finite`");

    assert_eq!(
        error.kind(),
        VocabularyErrorKind::MissingField { field: "finite" }
    );
}

#[test]
fn a_widened_bound_is_an_error_rather_than_a_silent_zero() {
    // uint32 -> uint64 is wire-compatible, so nothing else would notice; the
    // reader would return the type's default and switch the bound off.
    let pool = pool_with_vocabulary("minimal.proto", Some("wrongtype"));
    let error = Vocabulary::resolve(&pool).expect_err("max_items is unreadable as declared");

    assert_eq!(
        error.kind(),
        VocabularyErrorKind::WrongType {
            field: "max_items",
            expected: "uint32"
        }
    );
}

#[test]
fn the_canary_can_actually_fail() {
    // Without this, `check_canary` could be replaced with `Ok(())` and every
    // other test would still pass — leaving the one guard against silently
    // dropped option values unverified.
    let pool = pool_with_vocabulary("minimal.proto", Some("badcanary"));
    let error = Vocabulary::resolve(&pool).expect_err("the canary declares different values");

    assert!(matches!(
        error.kind(),
        VocabularyErrorKind::CanaryFailed { .. }
    ));
    assert_eq!(error.name(), "nv.telemetry.options.v1.Canary");
}

#[test]
fn missing_vocabulary_is_an_error_rather_than_an_absent_annotation() {
    let pool = pool_for("bare.proto");

    let error = Vocabulary::resolve(&pool).expect_err("vocabulary is not defined by this pool");

    // Without this, a dropped import or a renamed option would read as "no
    // field is annotated", turning every invariant off without a word.
    assert_eq!(error.name(), "nv.telemetry.options.v1.field_invariant");
    assert_eq!(error.kind(), VocabularyErrorKind::NotDefined);
}

#[test]
fn field_annotations_are_read_as_typed_values() {
    let (pool, vocabulary) = fixture("presence.proto");
    let message = pool
        .get_message_by_name("nv.telemetry.v1.PresenceFixture")
        .expect("fixture message is present");

    let field = |name: &str| {
        message
            .get_field_by_name(name)
            .expect("fixture field is present")
    };

    let declared_zero = vocabulary
        .field_invariant(&field("declared_zero"))
        .expect("annotation is present");
    assert!(declared_zero.zero_is_meaningful);
    assert!(!declared_zero.finite);

    let annotated = vocabulary
        .field_invariant(&field("annotated"))
        .expect("annotation is present");
    assert!(annotated.finite);
    assert!(!annotated.zero_is_meaningful);

    // An unset bound stays None, and a bound of zero stays Some(0). Collapsing
    // the two would reproduce, inside the compiler, the presence bug the
    // compiler exists to prevent.
    assert_eq!(annotated.max_items, None);
    assert_eq!(annotated.max_len, None);

    let empty = vocabulary
        .field_invariant(&field("must_be_empty"))
        .expect("annotation is present");
    assert_eq!(empty.max_items, Some(0));

    assert_eq!(vocabulary.field_invariant(&field("explicit")), None);
}

#[test]
fn message_annotations_are_read_as_typed_values() {
    let (pool, vocabulary) = fixture("presence.proto");

    let wrapper = pool
        .get_message_by_name("nv.telemetry.v1.WrapperFixture")
        .expect("fixture message is present");
    let invariant = vocabulary
        .message_invariant(&wrapper)
        .expect("annotation is present");
    assert!(invariant.validated);
    assert!(invariant.hashable);
    assert_eq!(invariant.max_depth, Some(4));

    let plain = pool
        .get_message_by_name("nv.telemetry.v1.PresenceFixture")
        .expect("fixture message is present");
    assert_eq!(vocabulary.message_invariant(&plain), None);
}

#[test]
fn shipped_contract_passes_its_own_lint() {
    let pool = nv_telemetry_codegen::pool().expect("shipped schema decodes");
    let vocabulary = Vocabulary::resolve(&pool).expect("shipped schema defines the vocabulary");

    assert_eq!(lint::presence(&pool, &vocabulary), Vec::new());
}
