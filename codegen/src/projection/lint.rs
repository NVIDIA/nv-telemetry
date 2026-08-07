// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Manifest rules: every declaration the compiler cannot honor is an error,
//! never silently degraded code — a mapping that reads as enforced while
//! doing nothing is the failure mode all of these guard.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt;

use heck::ToKebabCase as _;
use prost_reflect::DescriptorPool;
use prost_reflect::FieldDescriptor;
use prost_reflect::Kind;
use prost_reflect::MessageDescriptor;

use crate::is_contract_package;
use crate::options::Vocabulary;
use crate::projection::spec::AssemblySpec;
use crate::projection::spec::ConstantSpec;
use crate::projection::spec::FieldSpec;
use crate::projection::spec::ManifestSpec;
use crate::projection::spec::ProjectionSpec;
use crate::projection::spec::ScopeSpec;
use crate::projection::spec::SubjectSpec;
use crate::projection::RedfishIndex;
use crate::projection::ResolvedField;

// Enum numbers mirror manifest.proto; buf breaking guards the mirror.
const SCHEMA_INDEX: i32 = 1;
const NULL_UNSPECIFIED: i32 = 0;
const ELEMENTWISE: i32 = 2;

/// The one index this build can construct: nv-redfish's vendored DMTF bundle.
const DMTF_INDEX: &str = "nv-redfish-schema/dmtf";

/// The one target type a map assembly can build.
const VALUE: &str = "nv.telemetry.v1.Value";

/// One manifest declaration that breaks a rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    subject: String,
    reason: Reason,
}

impl Violation {
    /// The manifest file and projection at fault.
    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }

    #[must_use]
    pub fn reason(&self) -> &Reason {
        &self.reason
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.subject, self.reason)
    }
}

/// Why a manifest was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Reason {
    SourceMismatch { declared: String, directory: String },
    UnsupportedBackend,
    UnknownIndex { declared: String },
    Unnamed,
    DuplicateName(String),
    NotHonored { feature: &'static str },
    UnknownSourceType(String),
    UnknownSourcePath(String),
    UnknownTargetType(String),
    UnvalidatedTarget(String),
    UnknownTargetField(String),
    ReservedTarget(String),
    DuplicateTarget(String),
    OverlappingTargets { outer: String, inner: String },
    OneofConflict { first: String, second: String },
    MissingSubject,
    EmptyScopeSource,
    CaptureNotInTemplate { template: String, capture: String },
    NonScalarSubject { path: String, actual: String },
    SubjectNeverInherited,
    AnchorConflict(String),
    MultipleAnchors,
    UndecidedNull(String),
    EmptyValueMapping(String),
    DuplicateValueMapping { path: String, from: String },
    EmptySubjectKind,
    EmptyConstant(String),
    AssemblyTargetNotValue { field: String, actual: String },
    EmptyEntryKey,
    DuplicateEntryKey(String),
    UnknownEnumValue { path: String, value: String },
    UnresolvedPlaceholder(String),
    DuplicateMember(String),
    MembersWithoutVariation,
    ExpansionWithoutMembers,
}

impl fmt::Display for Reason {
    // One arm per reason; each is a diagnostic read under pressure.
    #[allow(clippy::too_many_lines)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceMismatch {
                declared,
                directory,
            } => write!(
                f,
                "`source: \"{declared}\"` but the manifest lives under \
                 `sources/{directory}/`; the two must agree"
            ),
            Self::UnsupportedBackend => f.write_str(
                "only BACKEND_SCHEMA_INDEX is supported; other backends have \
                 no resolver yet",
            ),
            Self::UnknownIndex { declared } => write!(
                f,
                "index `{declared}` is not one this build can construct; the \
                 available index is `{DMTF_INDEX}`"
            ),
            Self::Unnamed => f.write_str("every projection carries a name"),
            Self::DuplicateName(name) => {
                write!(f, "a second projection named `{name}`")
            }
            Self::NotHonored { feature } => write!(
                f,
                "`{feature}` is declared but the compiler does not implement \
                 it yet; generated code would silently ignore it"
            ),
            Self::UnknownSourceType(name) => write!(
                f,
                "source type `{name}` is not in the schema index; the \
                 projection would never match anything"
            ),
            Self::UnknownSourcePath(path) => write!(
                f,
                "`{path}` resolves to no field in the schema index; it would \
                 extract nothing and report nothing"
            ),
            Self::UnknownTargetType(name) => write!(
                f,
                "target `{name}` is not a message of the contract package"
            ),
            Self::UnvalidatedTarget(name) => write!(
                f,
                "target `{name}` is not `validated`; nothing would enforce \
                 what this projection produces"
            ),
            Self::UnknownTargetField(path) => write!(
                f,
                "target field `{path}` resolves to nothing in the target \
                 message"
            ),
            Self::ReservedTarget(path) => write!(
                f,
                "`{path}` writes into `subject`, which the subject \
                 declaration populates; identity never comes from a field \
                 mapping"
            ),
            Self::DuplicateTarget(path) => write!(
                f,
                "two declarations set `{path}`; whichever ran last would \
                 silently win"
            ),
            Self::OverlappingTargets { outer, inner } => write!(
                f,
                "`{outer}` is set whole while `{inner}` is set within it; \
                 the writes conflict"
            ),
            Self::OneofConflict { first, second } => write!(
                f,
                "`{first}` and `{second}` are cases of one oneof; setting \
                 both keeps only the last"
            ),
            Self::MissingSubject => f.write_str(
                "no subject: the projection would produce identity-less \
                 data, which joins to nothing",
            ),
            Self::EmptyScopeSource => f.write_str("a scope contributor carries no case"),
            Self::CaptureNotInTemplate { template, capture } => write!(
                f,
                "capture `{capture}` does not appear in `{template}` as \
                 `{{{capture}}}`"
            ),
            Self::NonScalarSubject { path, actual } => write!(
                f,
                "subject path `{path}` resolves to {actual}; an identity \
                 is one scalar value"
            ),
            Self::SubjectNeverInherited => f.write_str(
                "the manifest declares a subject, but every projection \
                 declares its own; the shared declaration is dead",
            ),
            Self::AnchorConflict(path) => write!(
                f,
                "`{path}` is both `anchor` and `required`: an absent anchor \
                 suppresses output silently while an absent required field \
                 reports, and one field cannot do both"
            ),
            Self::MultipleAnchors => f.write_str(
                "more than one `anchor`: the field a message exists to carry \
                 is singular",
            ),
            Self::UndecidedNull(path) => write!(
                f,
                "`{path}` is nullable at the source and declares no \
                 null_policy; what a null means is a decision, not a default"
            ),
            Self::EmptyValueMapping(path) => {
                write!(f, "`{path}` has a value mapping with an empty side")
            }
            Self::DuplicateValueMapping { path, from } => write!(
                f,
                "`{path}` maps `{from}` twice; whichever came last would \
                 silently win"
            ),
            Self::EmptySubjectKind => f.write_str("the subject has no kind, so it names nothing"),
            Self::EmptyConstant(field) => {
                write!(f, "constant for `{field}` is empty")
            }
            Self::AssemblyTargetNotValue { field, actual } => write!(
                f,
                "map assembly targets `{field}`, which is `{actual}`; \
                 assemblies build `{VALUE}` maps"
            ),
            Self::EmptyEntryKey => f.write_str("an assembly entry without a key"),
            Self::DuplicateEntryKey(key) => {
                write!(f, "a second assembly entry keyed `{key}`")
            }
            Self::UnknownEnumValue { path, value } => write!(
                f,
                "`{path}` names `{value}`, which the source enumeration does \
                 not declare; the row would never match anything"
            ),
            Self::UnresolvedPlaceholder(text) => write!(
                f,
                "`{text}` carries a brace placeholder nothing resolves; \
                 `{{member}}` and `{{member-kebab}}` substitute only in \
                 source paths and constant values inside an `expansion`"
            ),
            Self::DuplicateMember(member) => {
                write!(f, "member `{member}` is named twice")
            }
            Self::MembersWithoutVariation => f.write_str(
                "no source path or constant in the expansion varies by \
                 member; it would emit one projection several times",
            ),
            Self::ExpansionWithoutMembers => {
                f.write_str("an expansion without members expands nothing")
            }
        }
    }
}

/// Checks every manifest against the index and the contract pool.
#[must_use]
pub fn check(
    manifests: &[ManifestSpec],
    index: &RedfishIndex<'_>,
    contract: &DescriptorPool,
    vocabulary: &Vocabulary,
) -> Vec<Violation> {
    let mut violations = Vec::new();
    // Keyed by crate: a crate's manifest files share one emitted module,
    // so a name reused across files collides just as within one.
    let mut names = BTreeSet::new();
    for manifest in manifests {
        check_manifest(
            manifest,
            index,
            contract,
            vocabulary,
            &mut names,
            &mut violations,
        );
    }
    violations
}

fn check_manifest(
    manifest: &ManifestSpec,
    index: &RedfishIndex<'_>,
    contract: &DescriptorPool,
    vocabulary: &Vocabulary,
    names: &mut BTreeSet<(String, String)>,
    violations: &mut Vec<Violation>,
) {
    let file = manifest.path.display().to_string();
    let report = |violations: &mut Vec<Violation>, reason| {
        violations.push(Violation {
            subject: file.clone(),
            reason,
        });
    };

    if manifest.source != manifest.crate_source {
        report(
            violations,
            Reason::SourceMismatch {
                declared: manifest.source.clone(),
                directory: manifest.crate_source.clone(),
            },
        );
    }
    if manifest.backend != SCHEMA_INDEX {
        report(violations, Reason::UnsupportedBackend);
    }
    if manifest.index != DMTF_INDEX {
        report(
            violations,
            Reason::UnknownIndex {
                declared: manifest.index.clone(),
            },
        );
    }

    let inherited = manifest
        .projections
        .iter()
        .any(|projection| projection.subject.is_none());
    if manifest.subject.is_some() && !manifest.projections.is_empty() && !inherited {
        report(violations, Reason::SubjectNeverInherited);
    }

    for projection in &manifest.projections {
        let subject = format!("{file}: `{}`", projection.name);
        if projection.name.is_empty() {
            report(violations, Reason::Unnamed);
        } else if !names.insert((manifest.crate_source.clone(), projection.name.clone())) {
            report(violations, Reason::DuplicateName(projection.name.clone()));
        }
        Checker::check(
            projection,
            manifest.subject.as_ref(),
            &subject,
            index,
            contract,
            vocabulary,
            violations,
        );
    }
}

/// One projection's checking context: the source scope, the resolved
/// target, and the deduplicating violation sink. Expansion checks per
/// instance, but most faults do not vary by member; dedupe keeps one
/// declared fault one diagnostic. Path faults are judged only when the
/// source type itself is known, so an unknown type is one fault, not one
/// per path.
struct Checker<'a, 'b> {
    index: &'b RedfishIndex<'a>,
    source_type: &'b str,
    known: bool,
    target: Option<MessageDescriptor>,
    subject: &'b str,
    seen: BTreeSet<String>,
    violations: &'b mut Vec<Violation>,
}

impl<'a, 'b> Checker<'a, 'b> {
    fn check(
        projection: &'b ProjectionSpec,
        manifest_subject: Option<&SubjectSpec>,
        subject: &'b str,
        index: &'b RedfishIndex<'a>,
        contract: &DescriptorPool,
        vocabulary: &Vocabulary,
        violations: &'b mut Vec<Violation>,
    ) {
        let mut checker = Self {
            index,
            source_type: &projection.source_type,
            known: index.has_type(&projection.source_type),
            target: contract
                .get_message_by_name(&projection.target_type)
                .filter(|message| is_contract_package(message.package_name())),
            subject,
            seen: BTreeSet::new(),
            violations,
        };
        checker.run(projection, manifest_subject, vocabulary);
    }

    fn push(&mut self, reason: Reason) {
        if self.seen.insert(reason.to_string()) {
            self.violations.push(Violation {
                subject: self.subject.to_owned(),
                reason,
            });
        }
    }

    fn resolve(&self, path: &str) -> Option<ResolvedField> {
        if self.known {
            self.index.resolve(self.source_type, path)
        } else {
            None
        }
    }

    fn run(
        &mut self,
        projection: &ProjectionSpec,
        manifest_subject: Option<&SubjectSpec>,
        vocabulary: &Vocabulary,
    ) {
        if !projection.iterate.is_empty() {
            self.push(Reason::NotHonored { feature: "iterate" });
        }
        if projection.versions > 0 {
            self.push(Reason::NotHonored {
                feature: "versions",
            });
        }
        if !self.known {
            self.push(Reason::UnknownSourceType(self.source_type.to_owned()));
        }

        let validated = self.target.as_ref().map(|message| {
            vocabulary
                .message_invariant(message)
                .is_some_and(|invariant| invariant.validated)
        });
        match validated {
            None => self.push(Reason::UnknownTargetType(projection.target_type.clone())),
            Some(false) => self.push(Reason::UnvalidatedTarget(projection.target_type.clone())),
            Some(true) => {}
        }

        self.check_subject(projection.subject.as_ref().or(manifest_subject));

        for instance in self.expand(projection) {
            let mut anchors = 0usize;
            for field in &instance.fields {
                self.check_field(field);
                anchors += usize::from(field.anchor);
            }
            if anchors > 1 {
                self.push(Reason::MultipleAnchors);
            }

            for constant in &instance.constants {
                if constant.value.is_empty() {
                    self.push(Reason::EmptyConstant(constant.target_field.clone()));
                }
                self.check_target(&constant.target_field);
            }

            for assembly in &instance.map_assemblies {
                self.check_assembly(assembly);
            }

            self.check_targets(&instance);
        }
    }

    fn check_subject(&mut self, subject: Option<&SubjectSpec>) {
        let Some(spec) = subject else {
            self.push(Reason::MissingSubject);
            return;
        };
        if spec.kind.is_empty() {
            self.push(Reason::EmptySubjectKind);
        }
        self.check_subject_path(&spec.id_path);
        for contributor in &spec.scope {
            match contributor {
                ScopeSpec::PayloadPath(path) => {
                    self.check_subject_path(path);
                }
                ScopeSpec::LocationTemplate { template, capture } => {
                    if capture.is_empty() || !template.contains(&format!("{{{capture}}}")) {
                        self.push(Reason::CaptureNotInTemplate {
                            template: template.clone(),
                            capture: capture.clone(),
                        });
                    }
                }
                ScopeSpec::PathKey(_) => {
                    self.push(Reason::NotHonored {
                        feature: "path_key",
                    });
                }
                ScopeSpec::Unset => self.push(Reason::EmptyScopeSource),
            }
        }
    }

    /// A subject path must name one scalar value; a collection or a
    /// structured type cannot name a resource.
    fn check_subject_path(&mut self, path: &str) {
        if !self.known {
            return;
        }
        match self.resolve(path) {
            None => self.push(Reason::UnknownSourcePath(path.to_owned())),
            Some(resolved) if resolved.collection => self.push(Reason::NonScalarSubject {
                path: path.to_owned(),
                actual: "a collection".to_owned(),
            }),
            Some(resolved) if !resolved.is_scalar() => self.push(Reason::NonScalarSubject {
                path: path.to_owned(),
                actual: format!("complex type `{}`", resolved.type_name),
            }),
            Some(_) => {}
        }
    }

    /// One instance per member — the projection's shared declarations plus
    /// the expansion's, placeholders substituted — or the projection itself
    /// when it declares no expansion.
    fn expand(&mut self, projection: &ProjectionSpec) -> Vec<ProjectionSpec> {
        // Placeholders are meaningless outside an expansion.
        let shared = template_sites(
            &projection.fields,
            &projection.constants,
            &projection.map_assemblies,
        );
        for site in shared {
            if site.contains('{') {
                self.push(Reason::UnresolvedPlaceholder(site.to_owned()));
            }
        }
        let Some(expansion) = &projection.expansion else {
            return vec![projection.clone()];
        };

        if expansion.members.is_empty() {
            self.push(Reason::ExpansionWithoutMembers);
        }
        let mut seen = BTreeSet::new();
        for member in &expansion.members {
            if !seen.insert(member.as_str()) {
                self.push(Reason::DuplicateMember(member.clone()));
            }
        }
        let varying = template_sites(
            &expansion.fields,
            &expansion.constants,
            &expansion.map_assemblies,
        );
        if !varying.into_iter().any(|site| site.contains("{member")) {
            self.push(Reason::MembersWithoutVariation);
        }

        expansion
            .members
            .iter()
            .map(|member| {
                let mut instance = projection.clone();
                instance.expansion = None;
                for field in &expansion.fields {
                    let mut field = field.clone();
                    field.source_path = self.substituted(&field.source_path, member);
                    instance.fields.push(field);
                }
                for assembly in &expansion.map_assemblies {
                    let mut assembly = assembly.clone();
                    for entry in &mut assembly.entries {
                        entry.source_path = self.substituted(&entry.source_path, member);
                    }
                    instance.map_assemblies.push(assembly);
                }
                for constant in &expansion.constants {
                    let mut constant = constant.clone();
                    constant.value = self.substituted(&constant.value, member);
                    instance.constants.push(constant);
                }
                instance
            })
            .collect()
    }

    fn substituted(&mut self, text: &str, member: &str) -> String {
        let replaced = text
            .replace("{member-kebab}", &member.to_kebab_case())
            .replace("{member}", member);
        // Any brace left over is a placeholder that failed: a misspelling
        // would otherwise be emitted verbatim.
        if replaced.contains('{') {
            self.push(Reason::UnresolvedPlaceholder(text.to_owned()));
        }
        replaced
    }

    fn check_field(&mut self, field: &FieldSpec) {
        if let Some(resolved) =
            self.check_source(&field.source_path, field.null_policy, &field.value_map)
        {
            self.check_vocabulary(&field.source_path, &resolved, &field.known_values);
        }
        self.check_target(&field.target_field);
        if field.anchor && field.required {
            self.push(Reason::AnchorConflict(field.source_path.clone()));
        }
        if field.cardinality == ELEMENTWISE {
            self.push(Reason::NotHonored {
                feature: "CARDINALITY_ELEMENTWISE",
            });
        }
        if !field.unit.is_empty() {
            self.push(Reason::NotHonored { feature: "unit" });
        }
        if !field.unit_path.is_empty() {
            self.push(Reason::NotHonored {
                feature: "unit_path",
            });
        }
        self.check_value_map(&field.source_path, &field.value_map);
    }

    /// The checks every source path gets — resolution, collection support,
    /// a stated null policy, value-map vocabulary — shared by field
    /// mappings and assembly entries. Returns the resolution for
    /// caller-specific checks.
    fn check_source(
        &mut self,
        path: &str,
        null_policy: i32,
        value_map: &[(String, String)],
    ) -> Option<ResolvedField> {
        let Some(resolved) = self.resolve(path) else {
            if self.known {
                self.push(Reason::UnknownSourcePath(path.to_owned()));
            }
            return None;
        };
        if resolved.collection {
            self.push(Reason::NotHonored {
                feature: "collection-typed sources",
            });
        }
        if resolved.nullable && null_policy == NULL_UNSPECIFIED {
            self.push(Reason::UndecidedNull(path.to_owned()));
        }
        self.check_vocabulary(path, &resolved, value_map.iter().map(|(from, _)| from));
        Some(resolved)
    }

    /// Validates `value_map` sources and `known_values` against the
    /// resolved enum's members; non-enum sources have none to check.
    fn check_vocabulary(
        &mut self,
        path: &str,
        resolved: &ResolvedField,
        values: impl IntoIterator<Item = impl AsRef<str>>,
    ) {
        let Some(members) = &resolved.enum_members else {
            return;
        };
        for value in values {
            let value = value.as_ref();
            if !value.is_empty() && !members.iter().any(|member| member == value) {
                self.push(Reason::UnknownEnumValue {
                    path: path.to_owned(),
                    value: value.to_owned(),
                });
            }
        }
    }

    fn check_value_map(&mut self, path: &str, value_map: &[(String, String)]) {
        let mut froms = BTreeSet::new();
        for (from, to) in value_map {
            if from.is_empty() || to.is_empty() {
                self.push(Reason::EmptyValueMapping(path.to_owned()));
            }
            if !from.is_empty() && !froms.insert(from.clone()) {
                self.push(Reason::DuplicateValueMapping {
                    path: path.to_owned(),
                    from: from.clone(),
                });
            }
        }
    }

    fn check_assembly(&mut self, assembly: &AssemblySpec) {
        if let Some(leaf) = self.check_target(&assembly.target_field) {
            let actual = match leaf.kind() {
                Kind::Message(message) => message.full_name().to_owned(),
                other => format!("{other:?}"),
            };
            if actual != VALUE {
                self.push(Reason::AssemblyTargetNotValue {
                    field: assembly.target_field.clone(),
                    actual,
                });
            }
        }
        let mut keys = BTreeSet::new();
        for entry in &assembly.entries {
            if entry.key.is_empty() {
                self.push(Reason::EmptyEntryKey);
            } else if !keys.insert(entry.key.clone()) {
                self.push(Reason::DuplicateEntryKey(entry.key.clone()));
            }
            // Keys are never substituted; a brace is a placeholder gone
            // wrong.
            if entry.key.contains('{') {
                self.push(Reason::UnresolvedPlaceholder(entry.key.clone()));
            }
            self.check_source(&entry.source_path, entry.null_policy, &entry.value_map);
            self.check_value_map(&entry.source_path, &entry.value_map);
        }
    }

    /// Resolves a target path, reporting an unknown field or a repeated
    /// leaf, which no declaration can fill yet.
    fn check_target(&mut self, path: &str) -> Option<FieldDescriptor> {
        let leaf = resolve_target(self.target.as_ref()?, path);
        let Some(leaf) = leaf else {
            self.push(Reason::UnknownTargetField(path.to_owned()));
            return None;
        };
        if leaf.is_list() || leaf.is_map() {
            self.push(Reason::NotHonored {
                feature: "repeated target fields",
            });
        }
        Some(leaf)
    }

    /// Target fields must not collide: two writes to one path, a write
    /// inside a field another declaration sets whole, or two cases of one
    /// oneof.
    fn check_targets(&mut self, instance: &ProjectionSpec) {
        let declared: Vec<&str> = instance
            .fields
            .iter()
            .map(|field| field.target_field.as_str())
            .chain(
                instance
                    .constants
                    .iter()
                    .map(|constant| constant.target_field.as_str()),
            )
            .chain(
                instance
                    .map_assemblies
                    .iter()
                    .map(|assembly| assembly.target_field.as_str()),
            )
            .filter(|path| !path.is_empty())
            .collect();

        let mut seen = BTreeSet::new();
        for path in &declared {
            if !seen.insert(*path) {
                self.push(Reason::DuplicateTarget((*path).to_owned()));
            }
            if *path == "subject" || within("subject", path) {
                self.push(Reason::ReservedTarget((*path).to_owned()));
            }
        }
        for (index, first) in declared.iter().enumerate() {
            for second in &declared[index + 1..] {
                let (outer, inner) = if within(first, second) {
                    (first, second)
                } else if within(second, first) {
                    (second, first)
                } else {
                    continue;
                };
                self.push(Reason::OverlappingTargets {
                    outer: (*outer).to_owned(),
                    inner: (*inner).to_owned(),
                });
            }
        }

        let Some(target) = self.target.clone() else {
            return;
        };
        let mut cases: BTreeMap<(String, String), &str> = BTreeMap::new();
        for path in declared {
            let Some(leaf) = resolve_target(&target, path) else {
                continue;
            };
            let Some(oneof) = leaf.containing_oneof() else {
                continue;
            };
            let parent = path.rsplit_once('.').map_or("", |(parent, _)| parent);
            let key = (parent.to_owned(), oneof.name().to_owned());
            match cases.get(&key) {
                None => {
                    cases.insert(key, path);
                }
                Some(first) if *first != path => self.push(Reason::OneofConflict {
                    first: (*first).to_owned(),
                    second: path.to_owned(),
                }),
                Some(_) => {}
            }
        }
    }
}

/// Whether `inner` addresses a field inside the field at `outer`.
fn within(outer: &str, inner: &str) -> bool {
    inner
        .strip_prefix(outer)
        .is_some_and(|rest| rest.starts_with('.'))
}

/// Every string a member substitutes into: source paths and constant
/// values.
fn template_sites<'a>(
    fields: &'a [FieldSpec],
    constants: &'a [ConstantSpec],
    assemblies: &'a [AssemblySpec],
) -> impl Iterator<Item = &'a str> {
    let fields = fields.iter().map(|field| field.source_path.as_str());
    let entries = assemblies
        .iter()
        .flat_map(|assembly| assembly.entries.iter())
        .map(|entry| entry.source_path.as_str());
    let constants = constants.iter().map(|constant| constant.value.as_str());
    fields.chain(entries).chain(constants)
}

/// Resolves a dotted target path such as `range.min.double_value`; oneof
/// cases are fields of their message, so one walk covers both. Descent is
/// through singular messages only — an element of a repeated field is not
/// addressable.
fn resolve_target(message: &MessageDescriptor, path: &str) -> Option<FieldDescriptor> {
    let mut current = message.clone();
    let mut segments = path.split('.').peekable();
    while let Some(segment) = segments.next() {
        let field = current.get_field_by_name(segment)?;
        if segments.peek().is_none() {
            return Some(field);
        }
        if field.is_list() || field.is_map() {
            return None;
        }
        let Kind::Message(next) = field.kind() else {
            return None;
        };
        current = next;
    }
    None
}
