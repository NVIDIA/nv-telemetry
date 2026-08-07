// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Manifest rejections. The shipped sensor manifest is the passing case,
//! held green by `make check-codegen`; each test here pins one way a
//! declaration the compiler cannot honor fails loudly. The DMTF fold is
//! expensive, so one shared index serves every test.

use std::path::PathBuf;
use std::sync::LazyLock;

use nv_telemetry_codegen::options::Vocabulary;
use nv_telemetry_codegen::projection::check;
use nv_telemetry_codegen::projection::spec::AssemblySpec;
use nv_telemetry_codegen::projection::spec::ConstantSpec;
use nv_telemetry_codegen::projection::spec::EntrySpec;
use nv_telemetry_codegen::projection::spec::ExpansionSpec;
use nv_telemetry_codegen::projection::spec::FieldSpec;
use nv_telemetry_codegen::projection::spec::ManifestSpec;
use nv_telemetry_codegen::projection::spec::ProjectionSpec;
use nv_telemetry_codegen::projection::spec::ScopeSpec;
use nv_telemetry_codegen::projection::spec::SubjectSpec;
use nv_telemetry_codegen::projection::Bundle;
use nv_telemetry_codegen::projection::RedfishIndex;
use nv_telemetry_codegen::projection::Violation;
use prost_reflect::DescriptorPool;

const ABSENT: i32 = 1;

static POOL: LazyLock<DescriptorPool> =
    LazyLock::new(|| nv_telemetry_codegen::pool().expect("the shipped schema decodes"));
static VOCABULARY: LazyLock<Vocabulary> =
    LazyLock::new(|| Vocabulary::resolve(&POOL).expect("vocabulary resolves"));
static BUNDLE: LazyLock<Bundle> =
    LazyLock::new(|| Bundle::dmtf().expect("the vendored bundle parses"));
static INDEX: LazyLock<RedfishIndex<'static>> =
    LazyLock::new(|| BUNDLE.index().expect("the bundle indexes"));

fn violations_of(manifests: &[ManifestSpec]) -> Vec<Violation> {
    check(manifests, &INDEX, &POOL, &VOCABULARY)
}

fn rejects(spec: ManifestSpec, needle: &str) {
    let violations = violations_of(&[spec]);
    assert!(
        violations
            .iter()
            .any(|violation| violation.to_string().contains(needle)),
        "expected a violation mentioning {needle:?}, got: {violations:?}"
    );
}

fn passes(spec: ManifestSpec) {
    let violations = violations_of(&[spec]);
    assert!(
        violations.is_empty(),
        "unexpected violations: {violations:?}"
    );
}

fn subject() -> SubjectSpec {
    SubjectSpec {
        kind: "sensor".to_owned(),
        scope: vec![ScopeSpec::LocationTemplate {
            template: "/redfish/v1/Chassis/{chassis}/Sensors/{id}".to_owned(),
            capture: "chassis".to_owned(),
        }],
        id_path: "Id".to_owned(),
    }
}

fn field(source_path: &str, target_field: &str) -> FieldSpec {
    FieldSpec {
        source_path: source_path.to_owned(),
        target_field: target_field.to_owned(),
        required: false,
        anchor: false,
        unit: String::new(),
        unit_path: String::new(),
        known_values: Vec::new(),
        null_policy: ABSENT,
        cardinality: 0,
        value_map: Vec::new(),
    }
}

fn projection(name: &str) -> ProjectionSpec {
    ProjectionSpec {
        name: name.to_owned(),
        source_type: "Sensor".to_owned(),
        target_type: "nv.telemetry.v1.Reading".to_owned(),
        subject: Some(subject()),
        fields: vec![field("Reading", "value.double_value")],
        iterate: String::new(),
        versions: 0,
        constants: Vec::new(),
        map_assemblies: Vec::new(),
        expansion: None,
    }
}

fn manifest(projections: Vec<ProjectionSpec>) -> ManifestSpec {
    ManifestSpec {
        path: PathBuf::from("sources/redfish/manifests/test.textpb"),
        crate_source: "redfish".to_owned(),
        source: "redfish".to_owned(),
        backend: 1,
        index: "nv-redfish-schema/dmtf".to_owned(),
        projections,
        subject: None,
    }
}

/// A threshold-shaped projection: one anchored field at `path`, expanded
/// over `members`.
fn expanded(members: Vec<&str>, path: &str) -> ProjectionSpec {
    let mut threshold = field(path, "value.double_value");
    threshold.anchor = true;
    ProjectionSpec {
        target_type: "nv.telemetry.v1.StateObservation".to_owned(),
        constants: vec![ConstantSpec {
            target_field: "name".to_owned(),
            value: "threshold".to_owned(),
        }],
        fields: Vec::new(),
        expansion: Some(ExpansionSpec {
            members: members.into_iter().map(str::to_owned).collect(),
            fields: vec![threshold],
            constants: Vec::new(),
            map_assemblies: Vec::new(),
        }),
        ..projection("sample")
    }
}

// The baseline is clean, so each rejection below is its mutation's.
#[test]
fn the_baseline_manifest_is_clean() {
    passes(manifest(vec![projection("sample")]));
}

#[test]
fn an_unknown_source_path_is_rejected() {
    let mut broken = projection("sample");
    broken.fields[0].source_path = "Readng".to_owned();
    rejects(manifest(vec![broken]), "`Readng` resolves to no field");
}

#[test]
fn an_unknown_target_field_is_rejected() {
    let mut broken = projection("sample");
    broken.fields[0].target_field = "value.double".to_owned();
    rejects(manifest(vec![broken]), "target field `value.double`");
}

#[test]
fn anchor_and_required_cannot_meet_on_one_field() {
    let mut broken = projection("sample");
    broken.fields[0].anchor = true;
    broken.fields[0].required = true;
    rejects(manifest(vec![broken]), "both `anchor` and `required`");
}

#[test]
fn a_nullable_source_needs_a_null_policy() {
    // Reading is nullable in the DMTF schema.
    let mut broken = projection("sample");
    broken.fields[0].null_policy = 0;
    rejects(manifest(vec![broken]), "declares no null_policy");
}

#[test]
fn a_capture_must_appear_in_its_template() {
    let mut broken = projection("sample");
    broken.subject = Some(SubjectSpec {
        scope: vec![ScopeSpec::LocationTemplate {
            template: "/redfish/v1/Chassis/1U".to_owned(),
            capture: "chassis".to_owned(),
        }],
        ..subject()
    });
    rejects(manifest(vec![broken]), "does not appear");
}

#[test]
fn assemblies_build_value_maps_only() {
    let mut broken = projection("sample");
    broken.map_assemblies = vec![AssemblySpec {
        target_field: "key".to_owned(),
        entries: vec![EntrySpec {
            key: "reading".to_owned(),
            source_path: "Reading".to_owned(),
            null_policy: ABSENT,
            value_map: Vec::new(),
        }],
    }];
    rejects(manifest(vec![broken]), "assemblies build");
}

#[test]
fn iterate_is_not_honored_yet() {
    let mut broken = projection("sample");
    broken.iterate = "Members".to_owned();
    rejects(manifest(vec![broken]), "`iterate` is declared");
}

#[test]
fn an_unknown_source_type_reports_once() {
    let mut broken = projection("sample");
    broken.source_type = "Sensr".to_owned();
    let violations = violations_of(&[manifest(vec![broken])]);
    assert_eq!(
        violations.len(),
        1,
        "one root fault, no per-path echo: {violations:?}"
    );
    assert!(violations[0].to_string().contains("`Sensr`"));
}

#[test]
fn an_unknown_target_does_not_hide_a_source_fault() {
    let mut broken = projection("sample");
    broken.target_type = "nv.telemetry.v1.Nope".to_owned();
    broken.fields[0].source_path = "Readng".to_owned();
    let violations = violations_of(&[manifest(vec![broken])]);
    let text = format!("{violations:?}");
    assert!(text.contains("Nope"), "missing target fault: {text}");
    assert!(text.contains("Readng"), "missing source fault: {text}");
}

#[test]
fn a_value_mapping_from_outside_the_enum_is_rejected() {
    let mut broken = projection("sample");
    broken.fields[0].source_path = "ReadingType".to_owned();
    broken.fields[0].value_map = vec![("Temprature".to_owned(), "temperature".to_owned())];
    rejects(manifest(vec![broken]), "names `Temprature`");
}

#[test]
fn a_known_value_outside_the_enum_is_rejected() {
    let mut broken = projection("sample");
    broken.fields[0].source_path = "ReadingType".to_owned();
    broken.fields[0].known_values = vec!["Kelvinish".to_owned()];
    rejects(manifest(vec![broken]), "names `Kelvinish`");
}

#[test]
fn a_repeated_target_leaf_is_rejected() {
    let mut broken = projection("sample");
    broken.target_type = "nv.telemetry.v1.States".to_owned();
    broken.fields[0].target_field = "observations".to_owned();
    rejects(manifest(vec![broken]), "repeated target fields");
}

#[test]
fn descent_through_a_repeated_target_is_rejected() {
    let mut broken = projection("sample");
    broken.target_type = "nv.telemetry.v1.States".to_owned();
    broken.fields[0].target_field = "observations.name".to_owned();
    rejects(
        manifest(vec![broken]),
        "`observations.name` resolves to nothing",
    );
}

#[test]
fn two_writes_to_one_target_are_rejected() {
    let mut broken = projection("sample");
    broken
        .fields
        .push(field("ReadingUnits", "value.double_value"));
    rejects(manifest(vec![broken]), "two declarations set");
}

#[test]
fn a_write_inside_a_whole_set_field_is_rejected() {
    let mut broken = projection("sample");
    broken.constants = vec![ConstantSpec {
        target_field: "value".to_owned(),
        value: "x".to_owned(),
    }];
    rejects(manifest(vec![broken]), "is set whole while");
}

#[test]
fn two_cases_of_one_oneof_are_rejected() {
    let mut broken = projection("sample");
    broken.fields.push(field("SpeedRPM", "value.int_value"));
    rejects(manifest(vec![broken]), "cases of one oneof");
}

#[test]
fn the_subject_target_is_reserved() {
    let mut broken = projection("sample");
    broken.fields[0].target_field = "subject.kind".to_owned();
    rejects(manifest(vec![broken]), "subject declaration populates");

    let mut broken = projection("sample");
    broken.constants = vec![ConstantSpec {
        target_field: "subject".to_owned(),
        value: "sensor".to_owned(),
    }];
    rejects(manifest(vec![broken]), "subject declaration populates");
}

#[test]
fn a_manifest_subject_no_projection_inherits_is_dead() {
    let mut ignored = manifest(vec![projection("sample")]);
    ignored.subject = Some(subject());
    rejects(ignored, "shared declaration is dead");
}

#[test]
fn a_subject_path_names_one_scalar() {
    let mut broken = projection("sample");
    broken.subject = Some(SubjectSpec {
        id_path: "Status".to_owned(),
        ..subject()
    });
    rejects(manifest(vec![broken]), "an identity is one scalar value");

    let mut broken = projection("sample");
    broken.subject = Some(SubjectSpec {
        id_path: "Status.Conditions".to_owned(),
        ..subject()
    });
    rejects(manifest(vec![broken]), "resolves to a collection");
}

#[test]
fn projections_inherit_the_manifest_subject() {
    let mut inherited = projection("sample");
    inherited.subject = None;
    let mut with_default = manifest(vec![inherited]);
    with_default.subject = Some(subject());
    passes(with_default);
}

#[test]
fn a_projection_without_any_subject_is_rejected() {
    let mut orphan = projection("sample");
    orphan.subject = None;
    rejects(manifest(vec![orphan]), "no subject");
}

#[test]
fn placeholders_outside_an_expansion_are_rejected() {
    let mut broken = projection("sample");
    broken.fields[0].source_path = "Thresholds.{member}.Reading".to_owned();
    rejects(manifest(vec![broken]), "placeholder nothing resolves");
}

#[test]
fn a_misspelled_placeholder_names_its_site() {
    // The unresolved spelling both leaves a brace and fails resolution.
    rejects(
        manifest(vec![expanded(
            vec!["UpperCritical"],
            "Thresholds.{membr}.Reading",
        )]),
        "`Thresholds.{membr}.Reading`",
    );
}

#[test]
fn an_expansion_must_vary_by_member() {
    rejects(
        manifest(vec![expanded(vec!["UpperCritical"], "Thresholds.Reading")]),
        "no source path or constant",
    );
}

#[test]
fn a_duplicate_member_is_rejected() {
    rejects(
        manifest(vec![expanded(
            vec!["UpperCritical", "UpperCritical"],
            "Thresholds.{member}.Reading",
        )]),
        "named twice",
    );
}

#[test]
fn an_expansion_without_members_is_rejected() {
    rejects(
        manifest(vec![expanded(vec![], "Thresholds.{member}.Reading")]),
        "expands nothing",
    );
}

#[test]
fn expansion_feeds_real_resolution() {
    // The substituted path is what fails.
    rejects(
        manifest(vec![expanded(
            vec!["UpperBogus"],
            "Thresholds.{member}.Reading",
        )]),
        "`Thresholds.UpperBogus.Reading`",
    );
}

#[test]
fn a_fault_that_does_not_vary_is_one_diagnostic() {
    let members: Vec<&str> = vec!["M1", "M2", "M3", "M4", "M5", "M6"];
    let violations = violations_of(&[manifest(vec![expanded(
        members,
        "Thresholds.{member-keba}.Reading",
    )])]);
    let placeholders = violations
        .iter()
        .filter(|violation| violation.to_string().contains("placeholder"))
        .count();
    assert_eq!(placeholders, 1, "duplicate spam: {violations:?}");
}

#[test]
fn a_placeholder_typo_in_a_constant_is_caught() {
    // A constant is never resolved, so the leftover brace is the net.
    let mut typo = expanded(vec!["UpperCritical"], "Thresholds.{member}.Reading");
    typo.constants = Vec::new();
    typo.expansion.as_mut().expect("set above").constants = vec![ConstantSpec {
        target_field: "name".to_owned(),
        value: "threshold.{Member-kebab}".to_owned(),
    }];
    rejects(
        manifest(vec![typo]),
        "`threshold.{Member-kebab}` carries a brace",
    );
}

#[test]
fn an_entry_key_rejects_placeholders() {
    // Keys are never substituted; one would be emitted verbatim. The
    // fixture is otherwise clean, so this is its only violation.
    let mut braced = expanded(vec!["UpperCritical"], "Thresholds.{member}.Reading");
    let expansion = braced.expansion.as_mut().expect("set above");
    expansion.fields = Vec::new();
    expansion.map_assemblies = vec![AssemblySpec {
        target_field: "value".to_owned(),
        entries: vec![EntrySpec {
            key: "{member}".to_owned(),
            source_path: "Thresholds.{member}.Activation".to_owned(),
            null_policy: ABSENT,
            value_map: Vec::new(),
        }],
    }];
    let violations = violations_of(&[manifest(vec![braced])]);
    assert!(
        violations
            .iter()
            .any(|violation| violation.to_string().contains("`{member}` carries a brace")),
        "the key placeholder passed: {violations:?}"
    );
    assert_eq!(violations.len(), 1, "a noisy fixture: {violations:?}");
}

#[test]
fn projection_names_are_unique_per_crate() {
    // One emitted module per crate: a name reused across its manifest
    // files collides; another crate may reuse it freely.
    let twice = vec![
        manifest(vec![projection("sample")]),
        manifest(vec![projection("sample")]),
    ];
    let violations = violations_of(&twice);
    assert!(
        violations
            .iter()
            .any(|violation| violation.to_string().contains("a second projection named")),
        "cross-file duplicate passed: {violations:?}"
    );

    let mut other_crate = manifest(vec![projection("sample")]);
    other_crate.crate_source = "gnmi".to_owned();
    other_crate.source = "gnmi".to_owned();
    let pair = vec![manifest(vec![projection("sample")]), other_crate];
    let violations = violations_of(&pair);
    assert!(
        violations.is_empty(),
        "same name in another crate must pass: {violations:?}"
    );
}

#[test]
fn assembly_entries_get_the_same_source_checks_as_fields() {
    // A collection-typed entry source is not implemented and must not pass
    // silently.
    let mut broken = projection("sample");
    broken.target_type = "nv.telemetry.v1.StateObservation".to_owned();
    broken.fields = Vec::new();
    broken.constants = vec![ConstantSpec {
        target_field: "name".to_owned(),
        value: "conditions".to_owned(),
    }];
    broken.map_assemblies = vec![AssemblySpec {
        target_field: "value".to_owned(),
        entries: vec![EntrySpec {
            key: "conditions".to_owned(),
            source_path: "Status.Conditions".to_owned(),
            null_policy: ABSENT,
            value_map: Vec::new(),
        }],
    }];
    rejects(manifest(vec![broken]), "collection-typed sources");
}
