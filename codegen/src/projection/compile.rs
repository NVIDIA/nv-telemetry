// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The checked boundary between manifest semantics and Rust emission.
//!
//! This module owns every schema-dependent decision. The emitter receives
//! resolved source access, conversions, target assembly, identity, gates, and
//! names; it renders those facts but never consults a schema or manifest.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt;

use heck::ToSnakeCase as _;
use nv_redfish_csdl_compiler::compiler::TypeClass;
use nv_redfish_csdl_compiler::generator::casemungler;
use prost_reflect::DescriptorPool;
use prost_reflect::FieldDescriptor;
use prost_reflect::Kind;
use prost_reflect::MessageDescriptor;

use crate::options::Vocabulary;
use crate::projection::lint;
use crate::projection::lint::resolve_target;
use crate::projection::location::LocationPattern;
use crate::projection::spec::AssemblySpec;
use crate::projection::spec::EntrySpec;
use crate::projection::spec::ManifestSpec;
use crate::projection::spec::ProjectionSpec;
use crate::projection::spec::ScopeSpec;
use crate::projection::spec::SubjectSpec;
use crate::projection::RedfishIndex;
use crate::projection::Shape;
use crate::projection::Step;
use crate::projection::Violation;
use crate::provenance;
use crate::wrapper::names::constant_stem;
use crate::wrapper::names::short_name;

const NUMERIC_VALUE: &str = "nv.telemetry.v1.NumericValue";
const VALUE: &str = "nv.telemetry.v1.Value";
const SUBJECT: &str = "nv.telemetry.v1.Subject";

/// A target shape the projection compiler deliberately knows how to build.
///
/// Reflection verifies these declarations against the contract, but does not
/// infer new profiles. Adding a target is an architecture decision because it
/// decides how identity lands and which builder invariants emission promises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TargetProfile {
    pub(crate) target: &'static str,
    pub(crate) identity_field: &'static str,
    pub(crate) identity: IdentityKind,
    pub(crate) payload: &'static str,
    pub(crate) payload_field: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IdentityKind {
    SignalKey,
    Subject,
}

const TARGET_PROFILES: &[TargetProfile] = &[
    TargetProfile {
        target: "nv.telemetry.v1.SignalDescriptor",
        identity_field: "key",
        identity: IdentityKind::SignalKey,
        payload: "nv.telemetry.v1.Readings",
        payload_field: "descriptors",
    },
    TargetProfile {
        target: "nv.telemetry.v1.Reading",
        identity_field: "key",
        identity: IdentityKind::SignalKey,
        payload: "nv.telemetry.v1.Readings",
        payload_field: "samples",
    },
    TargetProfile {
        target: "nv.telemetry.v1.StateObservation",
        identity_field: "subject",
        identity: IdentityKind::Subject,
        payload: "nv.telemetry.v1.States",
        payload_field: "observations",
    },
];

#[must_use]
pub(crate) fn target_profile(target: &str) -> Option<TargetProfile> {
    TARGET_PROFILES
        .iter()
        .copied()
        .find(|profile| profile.target == target)
}

/// Fully lowered manifests. Private fields prevent callers from constructing
/// a value that skipped checking.
pub struct CompiledManifests {
    pub(crate) crates: Vec<CratePlan>,
}

impl fmt::Debug for CompiledManifests {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompiledManifests")
            .field("crates", &self.crates.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub(crate) struct CratePlan {
    pub(crate) crate_source: String,
    pub(crate) manifests: Vec<ManifestPlan>,
    pub(crate) provenance: Vec<provenance::Row>,
}

#[derive(Debug)]
pub(crate) struct ManifestPlan {
    pub(crate) relative_path: String,
    pub(crate) module: String,
    pub(crate) backend: BackendPlan,
    pub(crate) sources: Vec<SourcePlan>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum BackendPlan {
    Redfish,
}

#[derive(Debug)]
pub(crate) struct SourcePlan {
    pub(crate) source_type: String,
    pub(crate) source_namespace: String,
    pub(crate) parts_name: String,
    pub(crate) function_name: String,
    pub(crate) source_param: String,
    pub(crate) collections: Vec<CollectionPlan>,
    pub(crate) subject: SubjectPlan,
    pub(crate) instances: Vec<InstancePlan>,
    pub(crate) needs_key: bool,
}

#[derive(Debug)]
pub(crate) struct CollectionPlan {
    pub(crate) target_type: String,
    pub(crate) model_type: String,
    pub(crate) field: String,
}

#[derive(Debug)]
pub(crate) struct InstancePlan {
    pub(crate) evaluations: Vec<FieldEvaluationPlan>,
    pub(crate) groups: Vec<GroupBuildPlan>,
    pub(crate) outputs: Vec<OutputPlan>,
    pub(crate) constants: Vec<ConstantPlan>,
    pub(crate) assemblies: Vec<AssemblyPlan>,
    pub(crate) target_model: String,
    pub(crate) profile: TargetProfile,
    pub(crate) collection: String,
}

impl InstancePlan {
    fn always_emits(&self) -> bool {
        self.outputs.iter().all(|output| output.gates.is_empty()) && self.assemblies.is_empty()
    }
}

#[derive(Debug)]
pub(crate) struct FieldEvaluationPlan {
    pub(crate) local: String,
    pub(crate) read: ReadPlan,
    pub(crate) conversion: ConversionPlan,
}

#[derive(Clone, Debug)]
pub(crate) struct OutputPlan {
    pub(crate) local: String,
    pub(crate) setter: String,
    pub(crate) gates: Vec<GatePlan>,
    pub(crate) issue_path: String,
}

#[derive(Clone, Debug)]
pub(crate) enum GatePlan {
    Present(String),
    Flag(String),
}

#[derive(Debug)]
pub(crate) struct GroupBuildPlan {
    pub(crate) local: String,
    pub(crate) model_type: String,
    pub(crate) members: Vec<OutputPlan>,
    pub(crate) gate: Option<GroupGatePlan>,
    pub(crate) issue_path: String,
}

#[derive(Debug)]
pub(crate) struct GroupGatePlan {
    pub(crate) local: String,
    pub(crate) conditions: Vec<GatePlan>,
}

#[derive(Debug)]
pub(crate) struct ConstantPlan {
    pub(crate) setter: String,
    pub(crate) value: String,
    pub(crate) destination: TextDestination,
}

#[derive(Debug)]
pub(crate) struct AssemblyPlan {
    pub(crate) setter: String,
    pub(crate) local: String,
    pub(crate) entries: Vec<EntryPlan>,
}

#[derive(Debug)]
pub(crate) struct EntryPlan {
    pub(crate) key: String,
    pub(crate) read: ReadPlan,
    pub(crate) conversion: ConversionPlan,
}

#[derive(Clone, Debug)]
pub(crate) struct ReadPlan {
    pub(crate) steps: Vec<Step>,
    pub(crate) null_policy: NullPlan,
    pub(crate) required: bool,
    pub(crate) issue_path: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NullPlan {
    Absent,
    Invalid,
}

#[derive(Debug)]
pub(crate) enum ConversionPlan {
    DecimalValue {
        constructor: String,
    },
    Text {
        check: TextCheck,
        destination: TextDestination,
    },
    Enum {
        namespace: String,
        name: String,
        rows: Vec<(String, String)>,
        destination: TextDestination,
    },
}

#[derive(Clone, Debug)]
pub(crate) enum TextDestination {
    Plain,
    Vocabulary(String),
}

#[derive(Clone, Debug)]
pub(crate) struct TextCheck {
    pub(crate) field_name: String,
    pub(crate) non_empty: bool,
    pub(crate) max_len_constant: Option<String>,
}

#[derive(Debug)]
pub(crate) struct SubjectPlan {
    pub(crate) kind: String,
    pub(crate) id: SubjectIdPlan,
    pub(crate) scopes: Vec<SubjectScopePlan>,
    pub(crate) needs_location: bool,
}

#[derive(Debug)]
pub(crate) struct SubjectIdPlan {
    pub(crate) local: String,
    pub(crate) steps: Vec<Step>,
    pub(crate) issue_path: String,
    pub(crate) check: TextCheck,
}

#[derive(Debug)]
pub(crate) enum SubjectScopePlan {
    Payload {
        local: String,
        read: ReadPlan,
        check: TextCheck,
    },
    Location {
        local: String,
        helper: String,
        pattern: LocationPattern,
        check: TextCheck,
    },
}

struct LowerError {
    subject: String,
    detail: String,
}

type LowerResult<T> = Result<T, LowerError>;

/// Checks and lowers manifests into the only value emission accepts.
///
/// # Errors
///
/// Surface violations are aggregated in deterministic declaration order. If
/// that surface is clean, the first typed-lowering violation is returned.
/// Lowering failures are compile violations too; they cannot escape from
/// emission.
pub fn compile(
    manifests: &[ManifestSpec],
    index: &RedfishIndex<'_>,
    contract: &DescriptorPool,
    vocabulary: &Vocabulary,
) -> Result<CompiledManifests, Vec<Violation>> {
    let violations = lint::check(manifests, index, contract, vocabulary);
    if !violations.is_empty() {
        return Err(violations);
    }
    lower(manifests, index, contract, vocabulary)
        .map_err(|error| vec![Violation::cannot_emit(error.subject, error.detail)])
}

fn lower(
    manifests: &[ManifestSpec],
    index: &RedfishIndex<'_>,
    contract: &DescriptorPool,
    vocabulary: &Vocabulary,
) -> LowerResult<CompiledManifests> {
    let mut grouped: BTreeMap<&str, Vec<&ManifestSpec>> = BTreeMap::new();
    for manifest in manifests {
        grouped
            .entry(manifest.crate_source.as_str())
            .or_default()
            .push(manifest);
    }

    let mut crates = Vec::new();
    for (crate_source, manifests) in grouped {
        let provenance = provenance::collect(&manifests);
        let plans = manifests
            .iter()
            .map(|manifest| lower_manifest(manifest, index, contract, vocabulary))
            .collect::<LowerResult<Vec<_>>>()?;
        crates.push(CratePlan {
            crate_source: crate_source.to_owned(),
            manifests: plans,
            provenance,
        });
    }
    Ok(CompiledManifests { crates })
}

fn lower_manifest(
    manifest: &ManifestSpec,
    index: &RedfishIndex<'_>,
    contract: &DescriptorPool,
    vocabulary: &Vocabulary,
) -> LowerResult<ManifestPlan> {
    let relative_path = manifest.relative_path();
    let stem = manifest
        .path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let module = stem.to_snake_case();
    require_ident(&module, &relative_path, || {
        format!("file stem `{stem}` does not become a Rust module name")
    })?;
    let backend = match manifest.source.as_str() {
        "redfish" => BackendPlan::Redfish,
        other => {
            return fail(
                &relative_path,
                format!("no generated-type root is known for source `{other}`"),
            );
        }
    };

    let mut groups: Vec<(&str, Vec<&ProjectionSpec>)> = Vec::new();
    for projection in &manifest.projections {
        match groups
            .iter_mut()
            .find(|(source_type, _)| *source_type == projection.source_type)
        {
            Some((_, group)) => group.push(projection),
            None => groups.push((&projection.source_type, vec![projection])),
        }
    }
    let sources = groups
        .iter()
        .map(|(source_type, group)| {
            lower_source(manifest, source_type, group, index, contract, vocabulary)
        })
        .collect::<LowerResult<Vec<_>>>()?;
    Ok(ManifestPlan {
        relative_path,
        module,
        backend,
        sources,
    })
}

#[allow(clippy::too_many_lines)]
fn lower_source(
    manifest: &ManifestSpec,
    source_type: &str,
    group: &[&ProjectionSpec],
    index: &RedfishIndex<'_>,
    contract: &DescriptorPool,
    vocabulary: &Vocabulary,
) -> LowerResult<SourcePlan> {
    let manifest_path = manifest.relative_path();
    require_ident(source_type, &manifest_path, || {
        format!("source type `{source_type}` does not name Rust items")
    })?;
    let source_namespace = index
        .entity_namespace(source_type)
        .ok_or_else(|| LowerError {
            subject: manifest_path.clone(),
            detail: format!("source type `{source_type}` vanished from the index"),
        })?;
    let parts_name = format!("{source_type}Parts");
    let function_name = format!("project_{}", source_type.to_snake_case());
    let source_param = source_type.to_snake_case();

    let mut target_types: Vec<&str> = Vec::new();
    for projection in group {
        if !target_types.contains(&projection.target_type.as_str()) {
            target_types.push(&projection.target_type);
        }
    }
    let collections = target_types
        .iter()
        .map(|target_type| CollectionPlan {
            target_type: (*target_type).to_owned(),
            model_type: short_name(target_type),
            field: format!("{}s", short_name(target_type).to_snake_case()),
        })
        .collect::<Vec<_>>();

    let effective = group
        .iter()
        .map(|projection| {
            projection
                .subject
                .as_ref()
                .or(manifest.subject.as_ref())
                .ok_or_else(|| LowerError {
                    subject: format!("{manifest_path}: `{}`", projection.name),
                    detail: "no subject survived checking".to_owned(),
                })
        })
        .collect::<LowerResult<Vec<_>>>()?;
    let subject_spec = effective[0];
    if effective.iter().any(|subject| *subject != subject_spec) {
        return fail(
            &manifest_path,
            format!(
                "projections over `{source_type}` declare distinct subjects; one subject plan is required per source type"
            ),
        );
    }

    let mut locals: BTreeSet<String> = [
        "issues", "subject", "key", "location", "value", "error", "segments", "captured", "builder",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    locals.insert(source_param.clone());
    for collection in &collections {
        require_ident(&collection.field, &manifest_path, || {
            format!(
                "target `{}` derives an unusable parts field `{}`",
                collection.target_type, collection.field
            )
        })?;
        locals.insert(collection.field.clone());
    }

    let mut instances = Vec::new();
    let mut needs_key = false;
    for projection in group {
        let members = match &projection.expansion {
            Some(expansion) => expansion
                .members
                .iter()
                .map(|member| Some(member.as_str()))
                .collect(),
            None => vec![None],
        };
        for (instance, member) in projection.instances().into_iter().zip(members) {
            let mut prefix = projection.name.to_snake_case();
            if let Some(member) = member {
                prefix = format!("{prefix}_{}", casemungler::to_snake(member));
            }
            let context = format!("{manifest_path}: `{}`", projection.name);
            let target = contract
                .get_message_by_name(&instance.target_type)
                .ok_or_else(|| LowerError {
                    subject: context.clone(),
                    detail: format!("target `{}` vanished after checking", instance.target_type),
                })?;
            let profile = target_profile(&instance.target_type).ok_or_else(|| LowerError {
                subject: context.clone(),
                detail: format!(
                    "target `{}` lost its projection profile",
                    instance.target_type
                ),
            })?;

            let mut evaluations = Vec::new();
            let mut pending = Vec::new();
            for field in &instance.fields {
                let landing = target_landing(&target, &field.target_field, &context)?;
                let steps = source_steps(index, source_type, &field.source_path, &context)?;
                let issue_path = format!("{source_type}.{}", field.source_path);
                let conversion = conversion_plan(
                    steps.last().expect("a resolved path has a leaf"),
                    &landing,
                    &field.value_map,
                    &field.known_values,
                    vocabulary,
                    &context,
                )?;
                let local = claim_local(
                    &mut locals,
                    &format!(
                        "{}_{}",
                        prefix,
                        landing.setter_path.join("_").to_snake_case()
                    ),
                    &context,
                )?;
                let root = landing
                    .setter_path
                    .first()
                    .expect("a checked target landing has a root field");
                let root_field = target
                    .get_field_by_name(root)
                    .expect("a checked target root remains present");
                let target_required = vocabulary
                    .field_invariant(&root_field)
                    .is_some_and(|invariant| invariant.required);
                let gates = if field.anchor || field.required || target_required {
                    vec![GatePlan::Present(local.clone())]
                } else {
                    Vec::new()
                };
                evaluations.push(FieldEvaluationPlan {
                    local: local.clone(),
                    read: ReadPlan {
                        steps,
                        null_policy: lower_null_policy(field.null_policy, &context)?,
                        required: field.required,
                        issue_path: issue_path.clone(),
                    },
                    conversion,
                });
                pending.push(PendingOutput {
                    local,
                    setter_path: landing.setter_path,
                    gates,
                    issue_path,
                });
            }
            let (groups, outputs) = lower_groups(&target, &prefix, pending, &mut locals, &context)?;

            let constants = instance
                .constants
                .iter()
                .map(|constant| lower_constant(&target, constant, &context))
                .collect::<LowerResult<Vec<_>>>()?;
            let assemblies = instance
                .map_assemblies
                .iter()
                .map(|assembly| {
                    lower_assembly(
                        source_type,
                        &prefix,
                        assembly,
                        index,
                        contract,
                        vocabulary,
                        &mut locals,
                        &context,
                    )
                })
                .collect::<LowerResult<Vec<_>>>()?;
            let collection = collections
                .iter()
                .find(|collection| collection.target_type == instance.target_type)
                .expect("the instance's target collection is planned")
                .field
                .clone();
            needs_key |= profile.identity == IdentityKind::SignalKey;
            instances.push(InstancePlan {
                evaluations,
                groups,
                outputs,
                constants,
                assemblies,
                target_model: short_name(target.full_name()),
                profile,
                collection,
            });
        }
    }

    check_readings_payload(source_type, &instances, &manifest_path)?;
    check_payload_limits(
        source_type,
        &instances,
        contract,
        vocabulary,
        &manifest_path,
    )?;

    let subject = lower_subject(
        source_type,
        subject_spec,
        index,
        contract,
        vocabulary,
        &mut locals,
        &manifest_path,
    )?;
    Ok(SourcePlan {
        source_type: source_type.to_owned(),
        source_namespace,
        parts_name,
        function_name,
        source_param,
        collections,
        subject,
        instances,
        needs_key,
    })
}

struct PendingOutput {
    local: String,
    setter_path: Vec<String>,
    gates: Vec<GatePlan>,
    issue_path: String,
}

fn lower_groups(
    target: &MessageDescriptor,
    prefix: &str,
    pending: Vec<PendingOutput>,
    locals: &mut BTreeSet<String>,
    context: &str,
) -> LowerResult<(Vec<GroupBuildPlan>, Vec<OutputPlan>)> {
    let mut heads = Vec::new();
    for output in &pending {
        if output.setter_path.len() > 1 && !heads.contains(&output.setter_path[0]) {
            heads.push(output.setter_path[0].clone());
        }
    }
    if heads.is_empty() {
        return Ok((
            Vec::new(),
            pending
                .into_iter()
                .map(|output| OutputPlan {
                    local: output.local,
                    setter: output.setter_path[0].clone(),
                    gates: output.gates,
                    issue_path: output.issue_path,
                })
                .collect(),
        ));
    }

    let mut flat = Vec::new();
    let mut grouped: BTreeMap<String, Vec<PendingOutput>> = BTreeMap::new();
    for output in pending {
        if output.setter_path.len() > 1 {
            grouped
                .entry(output.setter_path[0].clone())
                .or_default()
                .push(output);
        } else {
            flat.push(OutputPlan {
                local: output.local,
                setter: output.setter_path[0].clone(),
                gates: output.gates,
                issue_path: output.issue_path,
            });
        }
    }

    let mut plans = Vec::new();
    for head in heads {
        let mut members = grouped.remove(&head).expect("the head was collected");
        let field = target.get_field_by_name(&head).ok_or_else(|| LowerError {
            subject: context.to_owned(),
            detail: format!("target field `{head}` vanished after checking"),
        })?;
        let Kind::Message(message) = field.kind() else {
            return fail(
                context,
                format!("`{head}` is not a message; nothing can be built within it"),
            );
        };
        for member in &mut members {
            member.setter_path.remove(0);
        }
        let sub_prefix = format!("{prefix}_{}", head.to_snake_case());
        let (mut nested, members) = lower_groups(&message, &sub_prefix, members, locals, context)?;
        plans.append(&mut nested);

        let local = claim_local(locals, &sub_prefix, context)?;
        let conditions = members
            .iter()
            .flat_map(|member| member.gates.iter().cloned())
            .collect::<Vec<_>>();
        let gate = if conditions.is_empty() {
            None
        } else {
            Some(GroupGatePlan {
                local: claim_local(locals, &format!("{sub_prefix}_gate"), context)?,
                conditions,
            })
        };
        let issue_path = members[0].issue_path.clone();
        let output_gates = gate
            .as_ref()
            .map_or_else(Vec::new, |gate| vec![GatePlan::Flag(gate.local.clone())]);
        plans.push(GroupBuildPlan {
            local: local.clone(),
            model_type: short_name(message.full_name()),
            members,
            gate,
            issue_path: issue_path.clone(),
        });
        flat.push(OutputPlan {
            local,
            setter: head,
            gates: output_gates,
            issue_path,
        });
    }
    Ok((plans, flat))
}

fn lower_constant(
    target: &MessageDescriptor,
    constant: &crate::projection::spec::ConstantSpec,
    context: &str,
) -> LowerResult<ConstantPlan> {
    let landing = target_landing(target, &constant.target_field, context)?;
    let [setter] = landing.setter_path.as_slice() else {
        return fail(
            context,
            format!(
                "constant for `{}` lands inside `{}`; group-building constants is not implemented",
                constant.target_field, landing.setter_path[0]
            ),
        );
    };
    let destination = text_destination(&landing.kind).ok_or_else(|| LowerError {
        subject: context.to_owned(),
        detail: format!(
            "a constant cannot populate `{}`",
            landing.kind.description()
        ),
    })?;
    Ok(ConstantPlan {
        setter: setter.clone(),
        value: constant.value.clone(),
        destination,
    })
}

#[allow(clippy::too_many_arguments)]
fn lower_assembly(
    source_type: &str,
    prefix: &str,
    assembly: &AssemblySpec,
    index: &RedfishIndex<'_>,
    contract: &DescriptorPool,
    vocabulary: &Vocabulary,
    locals: &mut BTreeSet<String>,
    context: &str,
) -> LowerResult<AssemblyPlan> {
    let segments = assembly.target_field.split('.').collect::<Vec<_>>();
    let [setter] = segments.as_slice() else {
        return fail(
            context,
            format!(
                "assembly target `{}` is nested; assemblies set a target root field",
                assembly.target_field
            ),
        );
    };
    let local = claim_local(
        locals,
        &format!(
            "{}_{}_entries",
            prefix,
            assembly.target_field.to_snake_case()
        ),
        context,
    )?;
    let entries = assembly
        .entries
        .iter()
        .map(|entry| lower_entry(source_type, entry, index, contract, vocabulary, context))
        .collect::<LowerResult<Vec<_>>>()?;
    Ok(AssemblyPlan {
        setter: (*setter).to_owned(),
        local,
        entries,
    })
}

fn lower_entry(
    source_type: &str,
    entry: &EntrySpec,
    index: &RedfishIndex<'_>,
    contract: &DescriptorPool,
    vocabulary: &Vocabulary,
    context: &str,
) -> LowerResult<EntryPlan> {
    let steps = source_steps(index, source_type, &entry.source_path, context)?;
    let leaf = steps.last().expect("a resolved path has a leaf");
    let arm = match source_class(leaf, context)? {
        SourceClass::Decimal => "double_value",
        SourceClass::Text | SourceClass::Enum => "string_value",
    };
    let field = contract
        .get_message_by_name(VALUE)
        .and_then(|message| message.get_field_by_name(arm))
        .ok_or_else(|| LowerError {
            subject: context.to_owned(),
            detail: format!("contract `{VALUE}.{arm}` vanished"),
        })?;
    let landing = LowerLanding {
        setter_path: Vec::new(),
        kind: LandingKind::VocabArm {
            vocabulary: VALUE.to_owned(),
            arm: arm.to_owned(),
            field,
        },
    };
    let conversion = conversion_plan(leaf, &landing, &entry.value_map, &[], vocabulary, context)?;
    Ok(EntryPlan {
        key: entry.key.clone(),
        read: ReadPlan {
            steps,
            null_policy: lower_null_policy(entry.null_policy, context)?,
            required: false,
            issue_path: format!("{source_type}.{}", entry.source_path),
        },
        conversion,
    })
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn lower_subject(
    source_type: &str,
    subject: &SubjectSpec,
    index: &RedfishIndex<'_>,
    contract: &DescriptorPool,
    vocabulary: &Vocabulary,
    locals: &mut BTreeSet<String>,
    manifest: &str,
) -> LowerResult<SubjectPlan> {
    let context = format!("{manifest}: subject");
    let id_steps = source_steps(index, source_type, &subject.id_path, &context)?;
    let id_leaf = id_steps.last().expect("a resolved path has a leaf");
    if id_steps.len() != 1 || id_leaf.shape() != Shape::Bare {
        return fail(
            &context,
            format!(
                "id path `{}` is not a required scalar on the source type itself; optional identity is not implemented",
                subject.id_path
            ),
        );
    }
    if source_class(id_leaf, &context)? != SourceClass::Text {
        return fail(
            &context,
            format!("id path `{}` is not a string", subject.id_path),
        );
    }
    let subject_message = contract
        .get_message_by_name(SUBJECT)
        .ok_or_else(|| LowerError {
            subject: context.clone(),
            detail: "contract Subject vanished".to_owned(),
        })?;
    let id_field = subject_message
        .get_field_by_name("id")
        .ok_or_else(|| LowerError {
            subject: context.clone(),
            detail: "contract Subject.id vanished".to_owned(),
        })?;
    let scope_field = subject_message
        .get_field_by_name("scope")
        .ok_or_else(|| LowerError {
            subject: context.clone(),
            detail: "contract Subject.scope vanished".to_owned(),
        })?;
    let id = SubjectIdPlan {
        local: claim_local(locals, "subject_id", &context)?,
        steps: id_steps,
        issue_path: format!("{source_type}.{}", subject.id_path),
        check: text_check(&id_field, vocabulary),
    };

    let mut scopes = Vec::new();
    let mut needs_location = false;
    for (position, contributor) in subject.scope.iter().enumerate() {
        let local = claim_local(locals, &format!("scope_{position}"), &context)?;
        match contributor {
            ScopeSpec::PayloadPath(path) => {
                let steps = source_steps(index, source_type, path, &context)?;
                let leaf = steps.last().expect("a resolved path has a leaf");
                if source_class(leaf, &context)? != SourceClass::Text {
                    return fail(&context, format!("scope path `{path}` is not a string"));
                }
                scopes.push(SubjectScopePlan::Payload {
                    local,
                    read: ReadPlan {
                        steps,
                        null_policy: NullPlan::Absent,
                        required: true,
                        issue_path: format!("{source_type}.{path}"),
                    },
                    check: text_check(&scope_field, vocabulary),
                });
            }
            ScopeSpec::LocationTemplate { template, capture } => {
                needs_location = true;
                let pattern =
                    LocationPattern::parse(template, capture).map_err(|detail| LowerError {
                        subject: context.clone(),
                        detail: format!(
                            "location template survived checking but could not be typed: {detail}"
                        ),
                    })?;
                scopes.push(SubjectScopePlan::Location {
                    local,
                    helper: format!("{}_subject_scope_{position}", source_type.to_snake_case()),
                    pattern,
                    check: text_check(&scope_field, vocabulary),
                });
            }
            ScopeSpec::PathKey(_) | ScopeSpec::Unset => {
                return fail(
                    &context,
                    "a scope contributor survived checking that the compiler cannot honor",
                );
            }
        }
    }
    Ok(SubjectPlan {
        kind: subject.kind.clone(),
        id,
        scopes,
        needs_location,
    })
}

fn lower_null_policy(value: i32, context: &str) -> LowerResult<NullPlan> {
    match value {
        0 | 1 => Ok(NullPlan::Absent),
        2 => Ok(NullPlan::Invalid),
        other => fail(
            context,
            format!("null policy {other} survived manifest checking"),
        ),
    }
}

/// Proves the invariants the provider relies on when it combines the two
/// generated collections into one `Readings` payload. All instances over a
/// source share one signal key, so more than one descriptor duplicates that
/// key; every possible sample also needs its descriptor to exist.
fn check_readings_payload(
    source_type: &str,
    instances: &[InstancePlan],
    context: &str,
) -> LowerResult<()> {
    let descriptors = instances
        .iter()
        .filter(|instance| instance.profile.target == "nv.telemetry.v1.SignalDescriptor")
        .collect::<Vec<_>>();
    let readings = instances
        .iter()
        .filter(|instance| instance.profile.target == "nv.telemetry.v1.Reading")
        .count();

    if descriptors.len() > 1 {
        return fail(
            context,
            format!(
                "source `{source_type}` expands to {} signal descriptors with the same key; a Readings payload requires descriptor keys to be unique",
                descriptors.len()
            ),
        );
    }
    if readings == 0 {
        return Ok(());
    }
    let [descriptor] = descriptors.as_slice() else {
        return fail(
            context,
            format!(
                "source `{source_type}` emits readings without exactly one signal descriptor; every sample key must resolve in its Readings payload"
            ),
        );
    };
    if !descriptor.always_emits() {
        return fail(
            context,
            format!(
                "source `{source_type}` gates its signal descriptor while a reading can emit; every possible sample must have a descriptor"
            ),
        );
    }
    Ok(())
}

/// Proves the repeated-field bounds the provider would otherwise discover
/// only when it assembles the generated vectors into payloads. The count is a
/// property of the static plan: each instance can contribute at most one
/// element, and a device that answers every gated field can realize that
/// maximum.
fn check_payload_limits(
    source_type: &str,
    instances: &[InstancePlan],
    contract: &DescriptorPool,
    vocabulary: &Vocabulary,
    context: &str,
) -> LowerResult<()> {
    for profile in TARGET_PROFILES {
        let actual = instances
            .iter()
            .filter(|instance| instance.profile.target == profile.target)
            .count();
        if actual == 0 {
            continue;
        }
        let payload = contract
            .get_message_by_name(profile.payload)
            .ok_or_else(|| LowerError {
                subject: context.to_owned(),
                detail: format!(
                    "profile payload `{}` vanished after checking",
                    profile.payload
                ),
            })?;
        let field = payload
            .get_field_by_name(profile.payload_field)
            .ok_or_else(|| LowerError {
                subject: context.to_owned(),
                detail: format!(
                    "profile payload field `{}.{}` vanished after checking",
                    profile.payload, profile.payload_field
                ),
            })?;
        let limit = vocabulary
            .field_invariant(&field)
            .and_then(|invariant| invariant.max_items)
            .ok_or_else(|| LowerError {
                subject: context.to_owned(),
                detail: format!(
                    "profile payload field `{}.{}` lost its max_items bound",
                    profile.payload, profile.payload_field
                ),
            })?;
        if let Some(detail) = payload_count_violation(source_type, *profile, actual, limit) {
            return fail(context, detail);
        }
    }
    Ok(())
}

fn payload_count_violation(
    source_type: &str,
    profile: TargetProfile,
    actual: usize,
    limit: u32,
) -> Option<String> {
    (actual > limit as usize).then(|| {
        format!(
            "source `{source_type}` can emit {actual} `{}` values into `{}.{}`, over the payload's max_items bound of {limit}",
            profile.target, profile.payload, profile.payload_field
        )
    })
}

struct LowerLanding {
    setter_path: Vec<String>,
    kind: LandingKind,
}

enum LandingKind {
    PlainString(FieldDescriptor),
    VocabArm {
        vocabulary: String,
        arm: String,
        field: FieldDescriptor,
    },
}

impl LandingKind {
    fn description(&self) -> String {
        match self {
            Self::PlainString(field) => field.full_name().to_owned(),
            Self::VocabArm {
                vocabulary, arm, ..
            } => format!("{vocabulary}.{arm}"),
        }
    }
}

fn target_landing(
    target: &MessageDescriptor,
    target_field: &str,
    context: &str,
) -> LowerResult<LowerLanding> {
    let leaf = resolve_target(target, target_field).ok_or_else(|| LowerError {
        subject: context.to_owned(),
        detail: format!("target field `{target_field}` vanished after checking"),
    })?;
    let segments = target_field.split('.').collect::<Vec<_>>();
    let parent = leaf.parent_message();
    if matches!(parent.full_name(), NUMERIC_VALUE | VALUE) {
        if segments.len() < 2 {
            return fail(
                context,
                format!("`{target_field}` names the value vocabulary itself"),
            );
        }
        return Ok(LowerLanding {
            setter_path: segments[..segments.len() - 1]
                .iter()
                .map(|segment| (*segment).to_owned())
                .collect(),
            kind: LandingKind::VocabArm {
                vocabulary: parent.full_name().to_owned(),
                arm: segments[segments.len() - 1].to_owned(),
                field: leaf,
            },
        });
    }
    match leaf.kind() {
        Kind::String => Ok(LowerLanding {
            setter_path: segments.iter().map(|segment| (*segment).to_owned()).collect(),
            kind: LandingKind::PlainString(leaf),
        }),
        other => fail(
            context,
            format!(
                "no conversion lands on `{target_field}` ({other:?}); extending the compiler is required"
            ),
        ),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceClass {
    Decimal,
    Text,
    Enum,
}

fn source_class(leaf: &Step, context: &str) -> LowerResult<SourceClass> {
    let primitive = match leaf.class {
        TypeClass::SimpleType => leaf.name.as_str(),
        TypeClass::TypeDefinition => leaf.underlying.as_deref().ok_or_else(|| LowerError {
            subject: context.to_owned(),
            detail: format!(
                "`{}.{}` has no underlying primitive",
                leaf.namespace, leaf.name
            ),
        })?,
        TypeClass::EnumType => return Ok(SourceClass::Enum),
        TypeClass::ComplexType => {
            return fail(
                context,
                format!(
                    "`{}` is a complex type; a mapping extracts one scalar",
                    leaf.property
                ),
            );
        }
    };
    if leaf.is_text() {
        return Ok(SourceClass::Text);
    }
    match primitive {
        "Decimal" => Ok(SourceClass::Decimal),
        other => fail(
            context,
            format!(
                "no conversion from `Edm.{other}` is implemented; extending the compiler is required"
            ),
        ),
    }
}

fn conversion_plan(
    leaf: &Step,
    landing: &LowerLanding,
    value_map: &[(String, String)],
    known_values: &[String],
    vocabulary: &Vocabulary,
    context: &str,
) -> LowerResult<ConversionPlan> {
    let class = source_class(leaf, context)?;
    match (&landing.kind, class) {
        (
            LandingKind::VocabArm {
                vocabulary, arm, ..
            },
            SourceClass::Decimal,
        ) if arm == "double_value" => Ok(ConversionPlan::DecimalValue {
            constructor: vocabulary.clone(),
        }),
        (kind, SourceClass::Text) => {
            let destination = text_destination(kind).ok_or_else(|| LowerError {
                subject: context.to_owned(),
                detail: format!(
                    "no conversion from `{}.{}` into `{}`",
                    leaf.namespace,
                    leaf.name,
                    kind.description()
                ),
            })?;
            Ok(ConversionPlan::Text {
                check: text_check(kind.field(), vocabulary),
                destination,
            })
        }
        (kind, SourceClass::Enum) => {
            let destination = text_destination(kind).ok_or_else(|| LowerError {
                subject: context.to_owned(),
                detail: format!(
                    "no conversion from `{}.{}` into `{}`",
                    leaf.namespace,
                    leaf.name,
                    kind.description()
                ),
            })?;
            let members = leaf.enum_members.as_ref().ok_or_else(|| LowerError {
                subject: context.to_owned(),
                detail: format!("`{}.{}` lost its members", leaf.namespace, leaf.name),
            })?;
            let mut rows = value_map.to_vec();
            for known in known_values {
                if !rows.iter().any(|(from, _)| from == known) {
                    rows.push((known.clone(), known.clone()));
                }
            }
            if rows.is_empty() {
                rows.extend(
                    members
                        .iter()
                        .map(|member| (member.clone(), member.clone())),
                );
            }
            Ok(ConversionPlan::Enum {
                namespace: leaf.namespace.clone(),
                name: leaf.name.clone(),
                rows,
                destination,
            })
        }
        (kind, SourceClass::Decimal) => fail(
            context,
            format!(
                "no conversion from `{}.{}` into `{}`",
                leaf.namespace,
                leaf.name,
                kind.description()
            ),
        ),
    }
}

impl LandingKind {
    fn field(&self) -> &FieldDescriptor {
        match self {
            Self::PlainString(field) | Self::VocabArm { field, .. } => field,
        }
    }
}

fn text_destination(kind: &LandingKind) -> Option<TextDestination> {
    match kind {
        LandingKind::PlainString(_) => Some(TextDestination::Plain),
        LandingKind::VocabArm {
            vocabulary, arm, ..
        } if arm == "string_value" => Some(TextDestination::Vocabulary(vocabulary.clone())),
        LandingKind::VocabArm { .. } => None,
    }
}

fn text_check(field: &FieldDescriptor, vocabulary: &Vocabulary) -> TextCheck {
    let invariant = vocabulary.field_invariant(field);
    TextCheck {
        field_name: field.name().to_owned(),
        non_empty: invariant
            .as_ref()
            .is_some_and(|invariant| invariant.non_empty),
        max_len_constant: invariant
            .and_then(|invariant| invariant.max_len)
            .map(|_| format!("{}_MAX_LEN", constant_stem(field.full_name()))),
    }
}

fn source_steps(
    index: &RedfishIndex<'_>,
    source_type: &str,
    path: &str,
    context: &str,
) -> LowerResult<Vec<Step>> {
    index.steps(source_type, path).map_err(|detail| LowerError {
        subject: context.to_owned(),
        detail,
    })
}

fn claim_local(locals: &mut BTreeSet<String>, name: &str, context: &str) -> LowerResult<String> {
    require_ident(name, context, || {
        format!(
            "derives the local name `{name}`, which is not a usable Rust identifier; rename the projection"
        )
    })?;
    if !locals.insert(name.to_owned()) {
        return fail(
            context,
            format!(
                "derives the local name `{name}` twice; rename a projection so generated locals stay distinct"
            ),
        );
    }
    Ok(name.to_owned())
}

fn require_ident(name: &str, subject: &str, detail: impl FnOnce() -> String) -> LowerResult<()> {
    if syn::parse_str::<syn::Ident>(name).is_err() {
        return fail(subject, detail());
    }
    Ok(())
}

fn fail<T>(subject: impl Into<String>, detail: impl Into<String>) -> LowerResult<T> {
    Err(LowerError {
        subject: subject.into(),
        detail: detail.into(),
    })
}

#[cfg(test)]
mod tests {
    use prost_reflect::Kind;

    use super::payload_count_violation;
    use super::target_profile;
    use super::TARGET_PROFILES;
    use crate::options::Vocabulary;

    #[test]
    fn payload_count_accepts_the_bound_and_rejects_the_next_instance() {
        let profile = target_profile("nv.telemetry.v1.Reading").expect("Reading is profiled");
        assert!(payload_count_violation("Sensor", profile, 65_536, 65_536).is_none());
        let violation = payload_count_violation("Sensor", profile, 65_537, 65_536)
            .expect("one element beyond max_items is rejected");
        assert!(violation.contains("65537"));
        assert!(violation.contains("Readings.samples"));
        assert!(violation.contains("65536"));
    }

    #[test]
    fn every_target_profile_lands_in_a_bounded_repeated_payload_field() {
        let pool = crate::pool().expect("the shipped contract decodes");
        let vocabulary = Vocabulary::resolve(&pool).expect("the vocabulary resolves");
        for profile in TARGET_PROFILES {
            let payload = pool
                .get_message_by_name(profile.payload)
                .unwrap_or_else(|| panic!("{} exists", profile.payload));
            let field = payload
                .get_field_by_name(profile.payload_field)
                .unwrap_or_else(|| panic!("{}.{} exists", profile.payload, profile.payload_field));
            assert!(
                field.is_list(),
                "{}.{} is repeated",
                profile.payload,
                profile.payload_field
            );
            let Kind::Message(element) = field.kind() else {
                panic!(
                    "{}.{} has a message element",
                    profile.payload, profile.payload_field
                );
            };
            assert_eq!(element.full_name(), profile.target);
            assert!(
                vocabulary
                    .field_invariant(&field)
                    .and_then(|invariant| invariant.max_items)
                    .is_some(),
                "{}.{} carries max_items",
                profile.payload,
                profile.payload_field
            );
        }
    }
}
