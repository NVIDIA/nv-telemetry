// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Validated model generation.
//!
//! The validated model is owned types: conversion consumes the wire message
//! field by field, and encoding rebuilds it. The alternative — newtypes over
//! the wire structs — cannot reshape, and reshaping is where the model's
//! value is: a required oneof becomes an enum rather than an `Option` of one,
//! a `required` string becomes `String` rather than `Option<String>`, and
//! absence is unrepresentable after conversion instead of checked at each
//! use.
//!
//! Construction is builder-only. A `new(required...)` constructor reads
//! better until the first message whose rules constrain only optional fields
//! — a `ValueRange` needs at least one of two optional bounds, so its `new()`
//! would always fail and the type would have no path to a valid value. A
//! builder with infallible setters and a fallible `build` handles every
//! message the same way, and `build` runs exactly the checks decoding runs,
//! so a built message and a decoded one cannot disagree about validity.
//!
//! The value vocabulary — `Value`, `NumericValue`, `Timestamp` — is
//! deliberately not generated. Its validated shapes are nothing like its wire
//! shapes (a recursive enum, a sorted map), and the [`limits`] module exists
//! so that hand-written code enforces the schema's bounds rather than a copy
//! of them.
//!
//! Cross-field rules the annotation vocabulary cannot express live in the
//! hand-written `rules` module, one function per generated message, and every
//! generated `check` calls its function. The registry is exhaustive by
//! compiler force: a new message does not compile until someone answers the
//! question "does it have cross-field rules?", in a file whose diff shows the
//! answer.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use prost_reflect::DescriptorPool;
use prost_reflect::FieldDescriptor;
use prost_reflect::Kind;
use prost_reflect::MessageDescriptor;

use crate::is_contract_package;
use crate::options::FieldInvariant;
use crate::options::Vocabulary;

/// Messages whose validated forms are hand-written in `model/src/value.rs`.
const HAND_WRITTEN: &[&str] = &[
    "nv.telemetry.v1.Null",
    "nv.telemetry.v1.Timestamp",
    "nv.telemetry.v1.NumericValue",
    "nv.telemetry.v1.Value",
    "nv.telemetry.v1.Value.List",
    "nv.telemetry.v1.Value.Map",
    "nv.telemetry.v1.Value.Map.Entry",
];

/// Types the hand-written vocabulary provides. Referenced by their short
/// names — the generated module imports them from the crate root.
fn hand_written_type(full_name: &str) -> Option<&'static str> {
    match full_name {
        "nv.telemetry.v1.Timestamp" => Some("Timestamp"),
        "nv.telemetry.v1.NumericValue" => Some("NumericValue"),
        "nv.telemetry.v1.Value" => Some("Value"),
        _ => None,
    }
}

const LIMITS_HEADER: &str = "\
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Generated from `nv.telemetry.v1` by `make codegen`. Do not edit.
//!
//! Every numeric bound the schema declares, one constant per annotation, so
//! validators — generated and hand-written alike — enforce the schema's
//! number rather than a copy of it. A bound changed in the schema reaches
//! every check by regeneration; a field renamed in the schema breaks its
//! hand-written consumers at compile time instead of leaving them checking a
//! bound that no longer exists.

// The whole contract's bounds are emitted whether or not anything consumes
// them yet, exactly as the wire types are.
#![allow(dead_code)]
";

/// Renders the limits module: one constant per bound the schema declares.
///
/// Constants are named after the declaration carrying the bound —
/// `VALUE_MAP_ENTRY_KEY_MAX_LEN` for `Value.Map.Entry.key`'s `max_len` — and
/// emitted in sorted order so the output is deterministic and a schema change
/// reads as a diff.
#[must_use]
pub fn limits(pool: &DescriptorPool, vocabulary: &Vocabulary) -> String {
    let mut constants: Vec<(String, String, u32)> = Vec::new();

    for message in pool.all_messages() {
        if !is_contract_package(message.package_name()) || message.is_map_entry() {
            continue;
        }

        if let Some(invariant) = vocabulary.message_invariant(&message) {
            if let Some(depth) = invariant.max_depth {
                constants.push((
                    format!("{}_MAX_DEPTH", constant_stem(message.full_name())),
                    format!("`{}` `max_depth`, in logical levels", message.full_name()),
                    depth,
                ));
            }
        }

        for field in message.fields() {
            let Some(invariant) = vocabulary.field_invariant(&field) else {
                continue;
            };
            if let Some(bound) = invariant.max_len {
                constants.push((
                    format!("{}_MAX_LEN", constant_stem(field.full_name())),
                    format!("`{}` `max_len`, in bytes", field.full_name()),
                    bound,
                ));
            }
            if let Some(bound) = invariant.max_items {
                constants.push((
                    format!("{}_MAX_ITEMS", constant_stem(field.full_name())),
                    format!("`{}` `max_items`", field.full_name()),
                    bound,
                ));
            }
        }
    }

    constants.sort();

    let mut out = String::from(LIMITS_HEADER);
    for (name, source, bound) in constants {
        let _ = write!(
            out,
            "\n/// {source}.\npub const {name}: u32 = {};\n",
            separated(bound)
        );
    }
    out
}

const MODEL_HEADER: &str = "\
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Generated from `nv.telemetry.v1` by `make codegen`. Do not edit.
//!
//! The validated model. Every type here upholds the schema's invariants for
//! as long as it exists: construction is a builder whose `build` validates,
//! decoding is `TryFrom` over the wire type running the same `check`, and the
//! fields are private so no path around either exists. Encoding rebuilds the
//! wire form, which is also where canonicalization shows: what comes out is
//! the validated representation, not the bytes that arrived.
//!
//! Absence is reshaped away where the schema says so. A `required` field is a
//! plain value, a required oneof is an enum, and an enum field cannot carry
//! the unspecified value — a value this build does not recognize decodes as
//! `Unrecognized`, because a newer producer naming something real is not an
//! error.

// Generated code holds the line on correctness lints; the pedantic group is
// style advice for humans and is exactly where a clippy release breaks a
// checked-in file that no one edited.
#![allow(clippy::pedantic, dead_code)]

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use crate::invalid;
use crate::rules;
use crate::value;
use crate::Invalid;
use crate::NumericValue;
use crate::Timestamp;
use crate::Value;
use crate::Violation;

use super::limits;
use super::wire;
";

/// Renders the validated model for every generated contract message.
///
/// # Errors
///
/// Returns a description naming the declaration if the contract contains a
/// shape this generator does not know how to reshape — which is a request to
/// extend the generator, not a schema error.
pub fn model(pool: &DescriptorPool, vocabulary: &Vocabulary) -> Result<String, String> {
    let mut out = String::from(MODEL_HEADER);

    let ordered = ord_needed(pool, vocabulary);
    let roots = root_messages(pool);

    let mut enums: Vec<_> = pool
        .all_enums()
        .filter(|declared| is_contract_package(declared.package_name()))
        .collect();
    enums.sort_by(|left, right| left.full_name().cmp(right.full_name()));
    for declared in enums {
        emit_enum(&mut out, &declared);
    }

    let mut messages: Vec<_> = pool
        .all_messages()
        .filter(|message| {
            is_contract_package(message.package_name())
                && !message.is_map_entry()
                && !HAND_WRITTEN.contains(&message.full_name())
        })
        .collect();
    messages.sort_by(|left, right| left.full_name().cmp(right.full_name()));

    for message in &messages {
        emit_message(
            &mut out,
            message,
            vocabulary,
            ordered.contains(message.full_name()),
            roots.contains(message.full_name()),
        )?;
    }

    Ok(out)
}

/// Generated messages needing `Ord` and `Hash`: every message type a
/// `unique_by` key resolves to, transitively, so uniqueness checks can hold
/// keys in a `BTreeSet` and consumers can use identities as map keys.
fn ord_needed(pool: &DescriptorPool, vocabulary: &Vocabulary) -> BTreeSet<String> {
    let mut needed = BTreeSet::new();
    for message in pool.all_messages() {
        if !is_contract_package(message.package_name()) {
            continue;
        }
        for field in message.fields() {
            let Some(invariant) = vocabulary.field_invariant(&field) else {
                continue;
            };
            if invariant.unique_by.is_empty() {
                continue;
            }
            let Kind::Message(element) = field.kind() else {
                continue;
            };
            for key in &invariant.unique_by {
                if let Some(member) = element.get_field_by_name(key) {
                    if let Kind::Message(key_type) = member.kind() {
                        collect_ord(&key_type, &mut needed);
                    }
                }
            }
        }
    }
    needed
}

fn collect_ord(message: &MessageDescriptor, needed: &mut BTreeSet<String>) {
    if !needed.insert(message.full_name().to_owned()) {
        return;
    }
    for field in message.fields() {
        if let Kind::Message(nested) = field.kind() {
            collect_ord(&nested, needed);
        }
    }
}

/// Messages nothing else in the contract holds: the boundary types, which get
/// `decode` and `encode_to_vec` because bytes are how they arrive and leave.
fn root_messages(pool: &DescriptorPool) -> BTreeSet<String> {
    let mut referenced = BTreeSet::new();
    for message in pool.all_messages() {
        if !is_contract_package(message.package_name()) {
            continue;
        }
        for field in message.fields() {
            if let Kind::Message(nested) = field.kind() {
                referenced.insert(nested.full_name().to_owned());
            }
        }
    }
    pool.all_messages()
        .filter(|message| {
            is_contract_package(message.package_name())
                && !message.is_map_entry()
                && !referenced.contains(message.full_name())
                && !HAND_WRITTEN.contains(&message.full_name())
        })
        .map(|message| message.full_name().to_owned())
        .collect()
}

fn emit_enum(out: &mut String, declared: &prost_reflect::EnumDescriptor) {
    let name = short_name(declared.full_name());
    let prefix = format!("{}_", screaming(&name));

    let _ = write!(
        out,
        "\n/// Validated form of `{full}`.\n\
         ///\n\
         /// The unspecified value is unrepresentable: conversion rejects it, because\n\
         /// every `{full}` field in the contract declares `reject_unspecified`. A value\n\
         /// newer than this build decodes as [`{name}::Unrecognized`] instead of\n\
         /// failing, so additive schema evolution does not break older consumers.\n\
         #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]\n\
         #[non_exhaustive]\n\
         pub enum {name} {{\n",
        full = declared.full_name(),
    );
    for entry in declared.values() {
        if entry.number() == 0 {
            continue;
        }
        let _ = write!(
            out,
            "    /// `{}`.\n    {},\n",
            entry.name(),
            arm_name(entry.name(), &prefix)
        );
    }
    out.push_str(
        "    /// A value newer than this build. Interpreting it is the consumer's\n\
         \x20   /// decision; re-encoding preserves it.\n\
         \x20   Unrecognized(i32),\n\
         }\n",
    );

    let _ = write!(
        out,
        "\nimpl TryFrom<i32> for {name} {{\n\
         \x20   type Error = Violation;\n\n\
         \x20   fn try_from(value: i32) -> Result<Self, Violation> {{\n\
         \x20       match value {{\n\
         \x20           0 => Err(Violation::Unspecified),\n"
    );
    for entry in declared.values() {
        if entry.number() == 0 {
            continue;
        }
        let _ = writeln!(
            out,
            "            {} => Ok(Self::{}),",
            entry.number(),
            arm_name(entry.name(), &prefix)
        );
    }
    out.push_str(
        "            other => Ok(Self::Unrecognized(other)),\n\
         \x20       }\n\
         \x20   }\n\
         }\n",
    );

    let _ = write!(
        out,
        "\nimpl From<{name}> for i32 {{\n\
         \x20   fn from(value: {name}) -> Self {{\n\
         \x20       match value {{\n"
    );
    for entry in declared.values() {
        if entry.number() == 0 {
            continue;
        }
        let _ = writeln!(
            out,
            "            {name}::{} => {},",
            arm_name(entry.name(), &prefix),
            entry.number()
        );
    }
    let _ = write!(
        out,
        "            {name}::Unrecognized(other) => other,\n\
         \x20       }}\n\
         \x20   }}\n\
         }}\n"
    );
}

/// Everything the generator needs to know about one field, computed once.
struct Plan {
    /// Field name; also the accessor, setter, and error-path name.
    name: String,
    /// Declaration in the validated struct.
    decl: String,
    /// Declaration in the builder.
    builder_decl: String,
    /// Setter body for the builder.
    setter: String,
    /// Expression building the validated field inside `build`, consuming
    /// `self.<name>`.
    build_init: String,
    /// Expression building the validated field inside `TryFrom`, consuming
    /// `wire.<name>`.
    from_wire: String,
    /// Expression rebuilding the wire field, consuming `value.<name>`.
    into_wire: String,
    /// Statements for `check`, referencing `self.<name>`.
    checks: String,
    /// The accessor.
    accessor: String,
}

// The message template in execution order — struct, impl, builder, TryFrom,
// From — and splitting it would scatter what is one shape.
#[allow(clippy::too_many_lines)]
fn emit_message(
    out: &mut String,
    message: &MessageDescriptor,
    vocabulary: &Vocabulary,
    ordered: bool,
    root: bool,
) -> Result<(), String> {
    let name = short_name(message.full_name());
    let rules_name = snake(&name);

    let mut plans = Vec::new();
    for field in message.fields() {
        if field
            .containing_oneof()
            .is_some_and(|oneof| !oneof.is_synthetic())
        {
            continue; // Emitted with the oneof.
        }
        plans.push(plan_field(&field, vocabulary)?);
    }

    let oneof = message
        .oneofs()
        .find(|oneof| !oneof.is_synthetic())
        .map(|oneof| plan_oneof(out, &oneof))
        .transpose()?;
    if let Some(oneof) = oneof {
        plans.push(oneof);
    }

    let derives = if ordered {
        "Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash"
    } else {
        "Clone, Debug, PartialEq, Eq"
    };

    // The struct.
    let _ = write!(
        out,
        "\n/// Validated form of `{full}`; the schema carries the field semantics.\n\
         ///\n\
         /// Holds its invariants for as long as it exists: built through\n\
         /// [`{name}Builder`] or decoded from the wire, both of which run the same\n\
         /// checks, including this message's cross-field rules.\n\
         #[derive({derives})]\n\
         pub struct {name} {{\n",
        full = message.full_name(),
    );
    for plan in &plans {
        let _ = writeln!(out, "    {},", plan.decl);
    }
    out.push_str("}\n");

    // Inherent impl: builder handle, accessors, check.
    let _ = write!(
        out,
        "\nimpl {name} {{\n\
         \x20   /// A builder holding nothing yet.\n\
         \x20   #[must_use]\n\
         \x20   pub fn builder() -> {name}Builder {{\n\
         \x20       {name}Builder::default()\n\
         \x20   }}\n"
    );
    for plan in &plans {
        out.push_str(&plan.accessor);
    }
    let _ = write!(out, "\n    fn check(&self) -> Result<(), Invalid> {{\n");
    for plan in &plans {
        out.push_str(&plan.checks);
    }
    let _ = write!(
        out,
        "        rules::{rules_name}(self)?;\n        Ok(())\n    }}\n"
    );
    if root {
        let _ = write!(
            out,
            "\n    /// Decodes and validates from wire bytes.\n\
             \x20   ///\n\
             \x20   /// # Errors\n\
             \x20   ///\n\
             \x20   /// [`DecodeError::Malformed`](crate::DecodeError) when the bytes are not\n\
             \x20   /// protobuf, [`DecodeError::Invalid`](crate::DecodeError) when they decode\n\
             \x20   /// but break the contract.\n\
             \x20   pub fn decode(bytes: &[u8]) -> Result<Self, crate::DecodeError> {{\n\
             \x20       let wire = <wire::{name} as ::prost::Message>::decode(bytes)\n\
             \x20           .map_err(crate::DecodeError::Malformed)?;\n\
             \x20       Self::try_from(wire).map_err(crate::DecodeError::Invalid)\n\
             \x20   }}\n\
             \n\
             \x20   /// Encodes the canonical wire form.\n\
             \x20   #[must_use]\n\
             \x20   pub fn encode_to_vec(&self) -> Vec<u8> {{\n\
             \x20       ::prost::Message::encode_to_vec(&wire::{name}::from(self.clone()))\n\
             \x20   }}\n"
        );
    }
    out.push_str("}\n");

    // The builder.
    let _ = write!(
        out,
        "\n/// Builds a [`{name}`]. Setters are infallible; [`build`]({name}Builder::build)\n\
         /// validates everything at once, exactly as decoding does.\n\
         #[derive(Clone, Debug, Default)]\n\
         pub struct {name}Builder {{\n"
    );
    for plan in &plans {
        let _ = writeln!(out, "    {},", plan.builder_decl);
    }
    out.push_str("}\n");

    let _ = write!(out, "\nimpl {name}Builder {{\n");
    for plan in &plans {
        out.push_str(&plan.setter);
    }
    let _ = write!(
        out,
        "\n    /// Validates and builds.\n\
         \x20   ///\n\
         \x20   /// # Errors\n\
         \x20   ///\n\
         \x20   /// [`Invalid`] naming the first field that is absent or breaks its\n\
         \x20   /// schema invariants.\n\
         \x20   pub fn build(self) -> Result<{name}, Invalid> {{\n\
         \x20       let built = {name} {{\n"
    );
    for plan in &plans {
        let _ = writeln!(out, "            {}: {},", plan.name, plan.build_init);
    }
    out.push_str(
        "        };\n\
         \x20       built.check()?;\n\
         \x20       Ok(built)\n\
         \x20   }\n\
         }\n",
    );

    // Decode path.
    let _ = write!(
        out,
        "\nimpl TryFrom<wire::{name}> for {name} {{\n\
         \x20   type Error = Invalid;\n\n\
         \x20   fn try_from(wire: wire::{name}) -> Result<Self, Invalid> {{\n\
         \x20       let built = Self {{\n"
    );
    for plan in &plans {
        let _ = writeln!(out, "            {}: {},", plan.name, plan.from_wire);
    }
    out.push_str(
        "        };\n\
         \x20       built.check()?;\n\
         \x20       Ok(built)\n\
         \x20   }\n\
         }\n",
    );

    // Encode path.
    let _ = write!(
        out,
        "\nimpl From<{name}> for wire::{name} {{\n\
         \x20   fn from(value: {name}) -> Self {{\n\
         \x20       Self {{\n"
    );
    for plan in &plans {
        let _ = writeln!(out, "            {}: {},", plan.name, plan.into_wire);
    }
    out.push_str(
        "        }\n\
         \x20   }\n\
         }\n",
    );

    Ok(())
}

/// Plans the payload oneof: emits its enum and returns the field plan the
/// containing message uses.
fn plan_oneof(out: &mut String, oneof: &prost_reflect::OneofDescriptor) -> Result<Plan, String> {
    let name = oneof.name().to_owned();
    let enum_name = camel(&name);
    let parent = short_name(oneof.parent_message().full_name());
    let wire_enum = format!("wire::{}::{}", snake(&parent), enum_name);

    let mut arms = Vec::new();
    for member in oneof.fields() {
        let Kind::Message(inner) = member.kind() else {
            return Err(format!(
                "`{}` has a scalar oneof member; this generator only reshapes \
                 message-typed oneofs",
                member.full_name()
            ));
        };
        arms.push((
            member.name().to_owned(),
            camel(member.name()),
            short_name(inner.full_name()),
        ));
    }

    let _ = write!(
        out,
        "\n/// The `{name}` of an `nv.telemetry.v1.{parent}`: exactly one case, always\n\
         /// set — the oneof is `required`, so absence is unrepresentable here.\n\
         #[derive(Clone, Debug, PartialEq, Eq)]\n\
         #[non_exhaustive]\n\
         pub enum {enum_name} {{\n"
    );
    for (field_name, arm, inner) in &arms {
        let _ = write!(out, "    /// `{field_name}`.\n    {arm}({inner}),\n");
    }
    out.push_str("}\n");

    let mut from_wire = format!(
        "match wire.{name}.ok_or_else(|| Invalid::field(\"{name}\", Violation::Absent))? {{\n"
    );
    for (field_name, arm, inner) in &arms {
        let _ = write!(
            from_wire,
            "                {wire_enum}::{arm}(inner) => {enum_name}::{arm}(\n\
             \x20                   {inner}::try_from(inner).map_err(|error| error.at(\"{field_name}\"))?,\n\
             \x20               ),\n"
        );
    }
    from_wire.push_str("            }");

    let mut into_wire = format!("Some(match value.{name} {{\n");
    for (_, arm, _) in &arms {
        let _ = writeln!(
            into_wire,
            "                {enum_name}::{arm}(inner) => {wire_enum}::{arm}(inner.into()),"
        );
    }
    into_wire.push_str("            })");

    Ok(Plan {
        decl: format!("{name}: {enum_name}"),
        builder_decl: format!("{name}: Option<{enum_name}>"),
        setter: format!(
            "    /// Sets `{name}`.\n\
             \x20   #[must_use]\n\
             \x20   pub fn {name}(mut self, {name}: {enum_name}) -> Self {{\n\
             \x20       self.{name} = Some({name});\n\
             \x20       self\n\
             \x20   }}\n"
        ),
        build_init: format!(
            "self.{name}.ok_or_else(|| Invalid::field(\"{name}\", Violation::Absent))?"
        ),
        from_wire,
        into_wire,
        accessor: format!(
            "\n    /// The `{name}`.\n\
             \x20   #[must_use]\n\
             \x20   pub fn {name}(&self) -> &{enum_name} {{\n\
             \x20       &self.{name}\n\
             \x20   }}\n"
        ),
        checks: String::new(),
        name,
    })
}

/// Plans one regular field.
// One match arm per field category, and the arms are what the function is:
// splitting each into its own function would hide that the categories differ
// only in the seven strings they produce.
#[allow(clippy::too_many_lines)]
fn plan_field(field: &FieldDescriptor, vocabulary: &Vocabulary) -> Result<Plan, String> {
    let invariant = vocabulary.field_invariant(field).unwrap_or_default();
    let name = field.name().to_owned();

    let absent = format!(".ok_or_else(|| Invalid::field(\"{name}\", Violation::Absent))?");

    let max_len = invariant
        .max_len
        .map(|_| format!("limits::{}_MAX_LEN", constant_stem(field.full_name())));
    let max_items = invariant
        .max_items
        .map(|_| format!("limits::{}_MAX_ITEMS", constant_stem(field.full_name())));

    let mut checks = String::new();

    let plan = match field.kind() {
        Kind::String if field.is_list() => {
            if let Some(limit) = &max_items {
                let _ = write!(
                    checks,
                    "        if let Some(violation) = invalid::too_many(self.{name}.len(), {limit}) {{\n\
                     \x20           return Err(Invalid::field(\"{name}\", violation));\n\
                     \x20       }}\n"
                );
            }
            if invariant.non_empty || max_len.is_some() {
                let _ = writeln!(
                    checks,
                    "        for (index, element) in self.{name}.iter().enumerate() {{"
                );
                if invariant.non_empty {
                    let _ = write!(
                        checks,
                        "            if element.is_empty() {{\n\
                         \x20               return Err(Invalid::element(\"{name}\", index, Violation::Empty));\n\
                         \x20           }}\n"
                    );
                }
                if let Some(limit) = &max_len {
                    let _ = write!(
                        checks,
                        "            if let Some(violation) = invalid::too_long(element.len(), {limit}) {{\n\
                         \x20               return Err(Invalid::element(\"{name}\", index, violation));\n\
                         \x20           }}\n"
                    );
                }
                checks.push_str("        }\n");
            }
            Plan {
                decl: format!("{name}: Vec<String>"),
                builder_decl: format!("{name}: Vec<String>"),
                setter: vec_setter(&name, "Vec<String>"),
                build_init: format!("self.{name}"),
                from_wire: format!("wire.{name}"),
                into_wire: format!("value.{name}"),
                accessor: slice_accessor(&name, "String"),
                checks,
                name: name.clone(),
            }
        }
        Kind::String => {
            let required = invariant.required;
            if invariant.non_empty {
                let target = if required {
                    format!("self.{name}")
                } else {
                    "element".to_owned()
                };
                let (open, indent, close) = optional_wrap(required, &name);
                let _ = write!(
                    checks,
                    "{open}{indent}        if {target}.is_empty() {{\n\
                     {indent}            return Err(Invalid::field(\"{name}\", Violation::Empty));\n\
                     {indent}        }}\n{close}"
                );
            }
            if let Some(limit) = &max_len {
                let target = if required {
                    format!("self.{name}")
                } else {
                    "element".to_owned()
                };
                let (open, indent, close) = optional_wrap(required, &name);
                let _ = write!(
                    checks,
                    "{open}{indent}        if let Some(violation) = invalid::too_long({target}.len(), {limit}) {{\n\
                     {indent}            return Err(Invalid::field(\"{name}\", violation));\n\
                     {indent}        }}\n{close}"
                );
            }
            if required {
                Plan {
                    decl: format!("{name}: String"),
                    builder_decl: format!("{name}: Option<String>"),
                    setter: into_string_setter(&name),
                    build_init: format!("self.{name}{absent}"),
                    from_wire: format!("wire.{name}{absent}"),
                    into_wire: format!("Some(value.{name})"),
                    accessor: str_accessor(&name, false),
                    checks,
                    name: name.clone(),
                }
            } else {
                Plan {
                    decl: format!("{name}: Option<String>"),
                    builder_decl: format!("{name}: Option<String>"),
                    setter: into_string_setter(&name),
                    build_init: format!("self.{name}"),
                    from_wire: format!("wire.{name}"),
                    into_wire: format!("value.{name}"),
                    accessor: str_accessor(&name, true),
                    checks,
                    name: name.clone(),
                }
            }
        }
        Kind::Bool => {
            if invariant.required {
                Plan {
                    decl: format!("{name}: bool"),
                    builder_decl: format!("{name}: Option<bool>"),
                    setter: value_setter(&name, "bool"),
                    build_init: format!("self.{name}{absent}"),
                    from_wire: format!("wire.{name}{absent}"),
                    into_wire: format!("Some(value.{name})"),
                    accessor: copy_accessor(&name, "bool", false),
                    checks,
                    name: name.clone(),
                }
            } else {
                Plan {
                    decl: format!("{name}: Option<bool>"),
                    builder_decl: format!("{name}: Option<bool>"),
                    setter: value_setter(&name, "bool"),
                    build_init: format!("self.{name}"),
                    from_wire: format!("wire.{name}"),
                    into_wire: format!("value.{name}"),
                    accessor: copy_accessor(&name, "bool", true),
                    checks,
                    name: name.clone(),
                }
            }
        }
        Kind::Uint64 => {
            if invariant.required {
                return Err(format!(
                    "`{}`: a required bare integer needs a reshaping decision \
                     this generator has not made yet",
                    field.full_name()
                ));
            }
            Plan {
                decl: format!("{name}: Option<u64>"),
                builder_decl: format!("{name}: Option<u64>"),
                setter: value_setter(&name, "u64"),
                build_init: format!("self.{name}"),
                from_wire: format!("wire.{name}"),
                into_wire: format!("value.{name}"),
                accessor: copy_accessor(&name, "u64", true),
                checks,
                name: name.clone(),
            }
        }
        Kind::Enum(declared) => {
            let enum_name = short_name(declared.full_name());
            if invariant.required {
                Plan {
                    decl: format!("{name}: {enum_name}"),
                    builder_decl: format!("{name}: Option<{enum_name}>"),
                    setter: value_setter(&name, &enum_name),
                    build_init: format!("self.{name}{absent}"),
                    from_wire: format!(
                        "{enum_name}::try_from(wire.{name}{absent})\n\
                         \x20               .map_err(|violation| Invalid::field(\"{name}\", violation))?"
                    ),
                    into_wire: format!("Some(value.{name}.into())"),
                    accessor: copy_accessor(&name, &enum_name, false),
                    checks,
                    name: name.clone(),
                }
            } else {
                Plan {
                    decl: format!("{name}: Option<{enum_name}>"),
                    builder_decl: format!("{name}: Option<{enum_name}>"),
                    setter: value_setter(&name, &enum_name),
                    build_init: format!("self.{name}"),
                    from_wire: format!(
                        "wire.{name}\n\
                         \x20               .map({enum_name}::try_from)\n\
                         \x20               .transpose()\n\
                         \x20               .map_err(|violation| Invalid::field(\"{name}\", violation))?"
                    ),
                    into_wire: format!("value.{name}.map(Into::into)"),
                    accessor: copy_accessor(&name, &enum_name, true),
                    checks,
                    name: name.clone(),
                }
            }
        }
        Kind::Message(inner) if inner.full_name() == "nv.telemetry.v1.Value.Map" => {
            let _ = write!(
                checks,
                "        if let Some(map) = &self.{name} {{\n\
                 \x20           value::check_map(map, \"{name}\")?;\n\
                 \x20       }}\n"
            );
            Plan {
                decl: format!("{name}: Option<BTreeMap<String, Value>>"),
                builder_decl: format!("{name}: Option<BTreeMap<String, Value>>"),
                setter: value_setter(&name, "BTreeMap<String, Value>"),
                build_init: format!("self.{name}"),
                from_wire: format!(
                    "wire.{name}\n\
                     \x20               .map(value::map_from_wire)\n\
                     \x20               .transpose()\n\
                     \x20               .map_err(|error| error.at(\"{name}\"))?"
                ),
                into_wire: format!("value.{name}.map(value::map_into_wire)"),
                accessor: format!(
                    "\n    /// The `{name}`, when present.\n\
                     \x20   #[must_use]\n\
                     \x20   pub fn {name}(&self) -> Option<&BTreeMap<String, Value>> {{\n\
                     \x20       self.{name}.as_ref()\n\
                     \x20   }}\n"
                ),
                checks,
                name: name.clone(),
            }
        }
        Kind::Message(inner) => {
            let rust = hand_written_type(inner.full_name())
                .map_or_else(|| short_name(inner.full_name()), ToOwned::to_owned);
            if field.is_list() {
                if let Some(limit) = &max_items {
                    let _ = write!(
                        checks,
                        "        if let Some(violation) = invalid::too_many(self.{name}.len(), {limit}) {{\n\
                         \x20           return Err(Invalid::field(\"{name}\", violation));\n\
                         \x20       }}\n"
                    );
                }
                if !invariant.unique_by.is_empty() {
                    let key = unique_key_expr(&invariant);
                    let _ = write!(
                        checks,
                        "        let mut seen = BTreeSet::new();\n\
                         \x20       for (index, element) in self.{name}.iter().enumerate() {{\n\
                         \x20           if !seen.insert({key}) {{\n\
                         \x20               return Err(Invalid::element(\"{name}\", index, Violation::Duplicate));\n\
                         \x20           }}\n\
                         \x20       }}\n"
                    );
                }
                Plan {
                    decl: format!("{name}: Vec<{rust}>"),
                    builder_decl: format!("{name}: Vec<{rust}>"),
                    setter: vec_setter(&name, &format!("Vec<{rust}>")),
                    build_init: format!("self.{name}"),
                    from_wire: format!(
                        "{{\n\
                         \x20               let mut elements = Vec::with_capacity(wire.{name}.len());\n\
                         \x20               for (index, element) in wire.{name}.into_iter().enumerate() {{\n\
                         \x20                   elements.push(\n\
                         \x20                       {rust}::try_from(element)\n\
                         \x20                           .map_err(|error| error.at_index(\"{name}\", index))?,\n\
                         \x20                   );\n\
                         \x20               }}\n\
                         \x20               elements\n\
                         \x20           }}"
                    ),
                    into_wire: format!("value.{name}.into_iter().map(Into::into).collect()"),
                    accessor: slice_accessor(&name, &rust),
                    checks,
                    name: name.clone(),
                }
            } else if invariant.required {
                Plan {
                    decl: format!("{name}: {rust}"),
                    builder_decl: format!("{name}: Option<{rust}>"),
                    setter: value_setter(&name, &rust),
                    build_init: format!("self.{name}{absent}"),
                    from_wire: format!(
                        "{rust}::try_from(wire.{name}{absent})\n\
                         \x20               .map_err(|error| error.at(\"{name}\"))?"
                    ),
                    into_wire: format!("Some(value.{name}.into())"),
                    accessor: ref_accessor(&name, &rust, false),
                    checks,
                    name: name.clone(),
                }
            } else {
                Plan {
                    decl: format!("{name}: Option<{rust}>"),
                    builder_decl: format!("{name}: Option<{rust}>"),
                    setter: value_setter(&name, &rust),
                    build_init: format!("self.{name}"),
                    from_wire: format!(
                        "wire.{name}\n\
                         \x20               .map({rust}::try_from)\n\
                         \x20               .transpose()\n\
                         \x20               .map_err(|error| error.at(\"{name}\"))?"
                    ),
                    into_wire: format!("value.{name}.map(Into::into)"),
                    accessor: ref_accessor(&name, &rust, true),
                    checks,
                    name: name.clone(),
                }
            }
        }
        other => {
            return Err(format!(
                "`{}`: this generator does not reshape `{other:?}` fields yet",
                field.full_name()
            ));
        }
    };

    Ok(plan)
}

/// The tuple of key references a uniqueness check inserts into its set.
fn unique_key_expr(invariant: &FieldInvariant) -> String {
    let keys: Vec<String> = invariant
        .unique_by
        .iter()
        .map(|key| format!("&element.{key}"))
        .collect();
    if keys.len() == 1 {
        keys.into_iter().next().unwrap_or_default()
    } else {
        format!("({})", keys.join(", "))
    }
}

/// Wrapping for a check on an optional field: `if let Some(element) = ...`.
fn optional_wrap(required: bool, name: &str) -> (String, &'static str, &'static str) {
    if required {
        (String::new(), "", "")
    } else {
        (
            format!("        if let Some(element) = &self.{name} {{\n"),
            "    ",
            "        }\n",
        )
    }
}

fn into_string_setter(name: &str) -> String {
    format!(
        "    /// Sets `{name}`.\n\
         \x20   #[must_use]\n\
         \x20   pub fn {name}(mut self, {name}: impl Into<String>) -> Self {{\n\
         \x20       self.{name} = Some({name}.into());\n\
         \x20       self\n\
         \x20   }}\n"
    )
}

fn value_setter(name: &str, ty: &str) -> String {
    format!(
        "    /// Sets `{name}`.\n\
         \x20   #[must_use]\n\
         \x20   pub fn {name}(mut self, {name}: {ty}) -> Self {{\n\
         \x20       self.{name} = Some({name});\n\
         \x20       self\n\
         \x20   }}\n"
    )
}

fn vec_setter(name: &str, ty: &str) -> String {
    format!(
        "    /// Sets `{name}`.\n\
         \x20   #[must_use]\n\
         \x20   pub fn {name}(mut self, {name}: {ty}) -> Self {{\n\
         \x20       self.{name} = {name};\n\
         \x20       self\n\
         \x20   }}\n"
    )
}

fn str_accessor(name: &str, optional: bool) -> String {
    if optional {
        format!(
            "\n    /// The `{name}`, when present.\n\
             \x20   #[must_use]\n\
             \x20   pub fn {name}(&self) -> Option<&str> {{\n\
             \x20       self.{name}.as_deref()\n\
             \x20   }}\n"
        )
    } else {
        format!(
            "\n    /// The `{name}`.\n\
             \x20   #[must_use]\n\
             \x20   pub fn {name}(&self) -> &str {{\n\
             \x20       &self.{name}\n\
             \x20   }}\n"
        )
    }
}

fn copy_accessor(name: &str, ty: &str, optional: bool) -> String {
    if optional {
        format!(
            "\n    /// The `{name}`, when present.\n\
             \x20   #[must_use]\n\
             \x20   pub fn {name}(&self) -> Option<{ty}> {{\n\
             \x20       self.{name}\n\
             \x20   }}\n"
        )
    } else {
        format!(
            "\n    /// The `{name}`.\n\
             \x20   #[must_use]\n\
             \x20   pub fn {name}(&self) -> {ty} {{\n\
             \x20       self.{name}\n\
             \x20   }}\n"
        )
    }
}

fn ref_accessor(name: &str, ty: &str, optional: bool) -> String {
    if optional {
        format!(
            "\n    /// The `{name}`, when present.\n\
             \x20   #[must_use]\n\
             \x20   pub fn {name}(&self) -> Option<&{ty}> {{\n\
             \x20       self.{name}.as_ref()\n\
             \x20   }}\n"
        )
    } else {
        format!(
            "\n    /// The `{name}`.\n\
             \x20   #[must_use]\n\
             \x20   pub fn {name}(&self) -> &{ty} {{\n\
             \x20       &self.{name}\n\
             \x20   }}\n"
        )
    }
}

fn slice_accessor(name: &str, ty: &str) -> String {
    format!(
        "\n    /// The `{name}`.\n\
         \x20   #[must_use]\n\
         \x20   pub fn {name}(&self) -> &[{ty}] {{\n\
         \x20       &self.{name}\n\
         \x20   }}\n"
    )
}

/// `nv.telemetry.v1.Value.Map.Entry.key` -> `VALUE_MAP_ENTRY_KEY`.
fn constant_stem(full_name: &str) -> String {
    full_name
        .strip_prefix(&format!("{}.", crate::CONTRACT_PACKAGE))
        .unwrap_or(full_name)
        .replace('.', "_")
        .to_ascii_uppercase()
}

/// `131072` -> `131_072`, matching what the workspace lints expect of a
/// literal a human reads.
fn separated(bound: u32) -> String {
    let digits = bound.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    let leading = digits.len() % 3;
    for (position, digit) in digits.chars().enumerate() {
        if position != 0 && position % 3 == leading % 3 {
            out.push('_');
        }
        out.push(digit);
    }
    out
}

/// `nv.telemetry.v1.AcquisitionStatus.FailureClass` -> `FailureClass`.
fn short_name(full_name: &str) -> String {
    full_name.rsplit('.').next().unwrap_or(full_name).to_owned()
}

/// `payload` -> `Payload`, `readings` -> `Readings`.
fn camel(snake_name: &str) -> String {
    snake_name
        .split('_')
        .map(|part| {
            let mut characters = part.chars();
            match characters.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// `SignalKey` -> `signal_key`.
fn snake(camel_name: &str) -> String {
    let mut out = String::with_capacity(camel_name.len() + 4);
    for (position, character) in camel_name.chars().enumerate() {
        if character.is_ascii_uppercase() && position != 0 {
            out.push('_');
        }
        out.push(character.to_ascii_lowercase());
    }
    out
}

/// `FailureClass` -> `FAILURE_CLASS`.
fn screaming(camel_name: &str) -> String {
    snake(camel_name).to_ascii_uppercase()
}

/// `FAILURE_CLASS_CONNECTIVITY` with prefix `FAILURE_CLASS_` -> `Connectivity`.
fn arm_name(value_name: &str, prefix: &str) -> String {
    camel(
        &value_name
            .strip_prefix(prefix)
            .unwrap_or(value_name)
            .to_ascii_lowercase(),
    )
}
