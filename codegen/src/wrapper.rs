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
//!
//! Emission builds `quote!` token streams, parses them into a file with
//! `syn`, and renders with `prettyplease`. Tokens rather than text because
//! Rust-emitting-Rust through `format!` doubles every brace and can fuse
//! adjacent tokens; the parse stays because `quote!` guarantees only lexical
//! well-formedness, and a template assembling tokens in a syntactically
//! impossible order must fail generation with a codegen error, not the model
//! crate's build. The file header is prepended verbatim: token streams have
//! no position for plain `//` comments, and the header's license and lint
//! rationale are exactly that. The `limits` module stays a text emitter — a
//! flat list of constants earns no tokens.
//!
//! Names the model cannot use as Rust identifiers — keyword field names, a
//! legal and common thing in protobuf — are rejected by the schema lint
//! before emission, so `format_ident!` here never meets one.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use proc_macro2::Ident;
use proc_macro2::Literal;
use proc_macro2::TokenStream;
use prost_reflect::DescriptorPool;
use prost_reflect::FieldDescriptor;
use prost_reflect::Kind;
use prost_reflect::MessageDescriptor;
use quote::format_ident;
use quote::quote;

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
    let mut items = TokenStream::new();

    let ordered = ord_needed(pool, vocabulary);
    let roots = root_messages(pool);

    // Every type name the module will contain, seeded with the names its
    // header already gives meaning: the hand-written vocabulary, the error
    // types, and the prelude types the generated code spells unqualified. A
    // derived name landing on one of these — a oneof called `value` becomes
    // `pub enum Value` beside the imported `Value` — is parseable, so without
    // this it fails in the model crate's build, blamed on a checked-in file.
    let mut claimed: BTreeSet<String> = [
        "Invalid",
        "Violation",
        "NumericValue",
        "Timestamp",
        "Value",
        "BTreeMap",
        "BTreeSet",
        "String",
        "Vec",
        "Option",
        "Result",
    ]
    .iter()
    .map(|&name| name.to_owned())
    .collect();

    let mut enums: Vec<_> = pool
        .all_enums()
        .filter(|declared| is_contract_package(declared.package_name()))
        .collect();
    enums.sort_by(|left, right| left.full_name().cmp(right.full_name()));
    for declared in enums {
        items.extend(enum_items(&declared, &mut claimed)?);
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
        items.extend(message_items(
            message,
            vocabulary,
            ordered.contains(message.full_name()),
            roots.contains(message.full_name()),
            &mut claimed,
        )?);
    }

    // `quote!` guarantees lexical well-formedness only; this is what proves
    // the assembled tokens are a Rust file. A stream that does not parse is a
    // bug in this module, and it must be reported here — as a generation
    // failure naming the parse error — rather than downstream as a broken
    // checked-in file.
    let parsed = syn::parse2::<syn::File>(items).map_err(|error| {
        format!(
            "the model emitter produced invalid Rust: {error}; this is a \
             codegen template bug, not a schema problem"
        )
    })?;

    let body = prettyplease::unparse(&parsed);
    // The uniqueness fallback is the one consumer of BTreeSet, and today's
    // contract never takes it: every unique collection is unordered with its
    // keys leading the canonical order, so the adjacent scan wins everywhere.
    // Import on demand rather than carry an allow for an import that is
    // usually dead.
    let set_import = if body.contains("BTreeSet") {
        "use std::collections::BTreeSet;\n"
    } else {
        ""
    };
    Ok(format!("{MODEL_HEADER}{set_import}\n{body}"))
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

/// Claims a derived type name, refusing one the module already uses.
fn claim(claimed: &mut BTreeSet<String>, name: &str, source: &str) -> Result<(), String> {
    if claimed.insert(name.to_owned()) {
        Ok(())
    } else {
        Err(format!(
            "{source} derives the type name `{name}`, which the generated \
             module already uses; rename it in the schema"
        ))
    }
}

/// `#[doc = " line"]` attributes from plain text lines, the token spelling of
/// `///` comments; prettyplease renders them back as `///`.
fn docs<S: AsRef<str>>(lines: &[S]) -> TokenStream {
    let attrs = lines.iter().map(|line| {
        let line = line.as_ref();
        let text = if line.is_empty() {
            String::new()
        } else {
            format!(" {line}")
        };
        quote! { #[doc = #text] }
    });
    quote! { #(#attrs)* }
}

fn ident(name: &str) -> Ident {
    format_ident!("{name}")
}

fn enum_items(
    declared: &prost_reflect::EnumDescriptor,
    claimed: &mut BTreeSet<String>,
) -> Result<TokenStream, String> {
    let full = declared.full_name();
    let short = short_name(full);
    claim(claimed, &short, &format!("enum `{full}`"))?;
    let name = ident(&short);
    let prefix = format!("{}_", screaming(&short));

    let doc = docs(&[
        format!("Validated form of `{full}`."),
        String::new(),
        "The unspecified value is unrepresentable: conversion rejects it, because".to_owned(),
        format!("every `{full}` field in the contract declares `reject_unspecified`. A value"),
        format!("newer than this build decodes as [`{short}::Unrecognized`] instead of"),
        "failing, so additive schema evolution does not break older consumers.".to_owned(),
    ]);
    let unrecognized_doc = docs(&[
        "A value newer than this build. Interpreting it is the consumer's",
        "decision; re-encoding preserves it.",
    ]);

    // Variant names are derived, so two schema names can reduce to one Rust
    // name — `X_A_B` and `X_A__B` both become `AB` — and the generator itself
    // claims `Unrecognized`. A duplicate variant is still parseable Rust, so
    // without this check it would fail in the model crate's build, attributed
    // to a checked-in file rather than to the schema name that caused it.
    let mut taken = BTreeSet::from(["Unrecognized".to_owned()]);
    let mut arms = Vec::new();
    for entry in declared.values() {
        if entry.number() == 0 {
            continue;
        }
        let arm = arm_name(entry.name(), &prefix);
        // Deriving can produce something Rust cannot spell — stripping the
        // enum prefix from `FORM_FACTOR_2U` leaves a variant starting with a
        // digit — and `format_ident!` would panic on it, a generator crash
        // where a schema error is owed.
        if syn::parse_str::<Ident>(&arm).is_err() {
            return Err(format!(
                "enum `{full}` value `{}` derives the variant name `{arm}`, \
                 which is not a usable Rust identifier; rename the value in \
                 the schema",
                entry.name()
            ));
        }
        if !taken.insert(arm.clone()) {
            return Err(format!(
                "enum `{full}` value `{}` becomes the variant `{arm}`, which \
                 another value — or the generator's own `Unrecognized` arm — \
                 already uses; rename the value in the schema",
                entry.name()
            ));
        }
        arms.push((
            ident(&arm),
            Literal::i32_unsuffixed(entry.number()),
            entry.name().to_owned(),
        ));
    }

    let variants = arms.iter().map(|(arm, _, value_name)| {
        let doc = docs(&[format!("`{value_name}`.")]);
        quote! { #doc #arm, }
    });
    let decode_arms = arms.iter().map(|(arm, number, _)| {
        quote! { #number => Ok(Self::#arm), }
    });
    let encode_arms = arms.iter().map(|(arm, number, _)| {
        quote! { #name::#arm => #number, }
    });

    Ok(quote! {
        #doc
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[non_exhaustive]
        pub enum #name {
            #(#variants)*
            #unrecognized_doc
            Unrecognized(i32),
        }

        impl TryFrom<i32> for #name {
            type Error = Violation;

            fn try_from(value: i32) -> Result<Self, Violation> {
                match value {
                    0 => Err(Violation::Unspecified),
                    #(#decode_arms)*
                    other => Ok(Self::Unrecognized(other)),
                }
            }
        }

        impl From<#name> for i32 {
            fn from(value: #name) -> Self {
                match value {
                    #(#encode_arms)*
                    #name::Unrecognized(other) => other,
                }
            }
        }

        impl crate::canonical::Canonical for #name {
            fn canonical_cmp(&self, other: &Self) -> std::cmp::Ordering {
                i32::from(*self).cmp(&i32::from(*other))
            }
        }
    })
}

/// Everything the generator needs to know about one field, computed once.
struct Plan {
    /// Field name as an identifier; also names the accessor and setter.
    ident: Ident,
    /// Field number: the canonical order compares and the digest labels by
    /// it, never by declaration order.
    number: u32,
    /// Collection metadata: skipped by the digest, compared only in the
    /// canonical order's tiebreak phase.
    metadata: bool,
    /// Comparison expression against `other` for the canonical order.
    cmp: TokenStream,
    /// Statements feeding this field to the digest.
    digest: TokenStream,
    /// Declared type in the validated struct.
    decl_ty: TokenStream,
    /// Declared type in the builder.
    builder_ty: TokenStream,
    /// The builder's setter method.
    setter: TokenStream,
    /// Expression building the validated field inside `build`, consuming
    /// `self.<name>`.
    build_init: TokenStream,
    /// Expression building the validated field inside `TryFrom`, consuming
    /// `wire.<name>`.
    from_wire: TokenStream,
    /// Expression rebuilding the wire field, consuming `value.<name>`.
    into_wire: TokenStream,
    /// Statements for `check`, referencing `self.<name>`.
    checks: TokenStream,
    /// The accessor method.
    accessor: TokenStream,
}

// The message template in execution order — struct, impl, builder, TryFrom,
// From — and splitting it would scatter what is one shape.
#[allow(clippy::too_many_lines)]
fn message_items(
    message: &MessageDescriptor,
    vocabulary: &Vocabulary,
    ordered: bool,
    root: bool,
    claimed: &mut BTreeSet<String>,
) -> Result<TokenStream, String> {
    let full = message.full_name();
    let short = short_name(full);
    claim(claimed, &short, &format!("message `{full}`"))?;
    claim(
        claimed,
        &format!("{short}Builder"),
        &format!("message `{full}`'s builder"),
    )?;
    let name = ident(&short);
    let builder = format_ident!("{short}Builder");
    // `Match` is a legal, styled message name whose registry function would
    // be `fn match`; `syn` rejects keywords as identifiers, so this is also
    // where that surfaces as a schema error rather than a parse failure
    // blamed on the templates.
    let rules_name = snake(&short);
    if syn::parse_str::<Ident>(&rules_name).is_err() {
        return Err(format!(
            "message `{full}`'s cross-field rules function would be named \
             `{rules_name}`, which Rust reserves; rename the message in the \
             schema"
        ));
    }
    let rules_fn = ident(&rules_name);

    let mut items = TokenStream::new();

    let mut plans = Vec::new();
    for field in message.fields() {
        if field
            .containing_oneof()
            .is_some_and(|oneof| !oneof.is_synthetic())
        {
            continue; // Planned with the oneof.
        }
        plans.push(plan_field(&field, vocabulary)?);
    }
    for oneof in message.oneofs().filter(|oneof| !oneof.is_synthetic()) {
        let (payload_enum, plan) = plan_oneof(&oneof, vocabulary, claimed)?;
        items.extend(payload_enum);
        plans.push(plan);
    }

    let derive = if ordered {
        quote! { #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)] }
    } else {
        quote! { #[derive(Clone, Debug, PartialEq, Eq)] }
    };

    let struct_doc = docs(&[
        format!("Validated form of `{full}`; the schema carries the field semantics."),
        String::new(),
        "Holds its invariants for as long as it exists: built through".to_owned(),
        format!("[`{short}Builder`] or decoded from the wire, both of which run the same"),
        "checks, including this message's cross-field rules.".to_owned(),
    ]);
    let builder_doc = docs(&[
        format!("Builds a [`{short}`]. Setters are infallible; [`build`]({short}Builder::build)"),
        "validates everything at once, exactly as decoding does.".to_owned(),
    ]);
    let builder_fn_doc = docs(&["A builder holding nothing yet."]);
    let build_doc = docs(&[
        "Validates and builds.",
        "",
        "# Errors",
        "",
        "[`Invalid`] naming the first field that is absent or breaks its",
        "schema invariants.",
    ]);

    let codec = root.then(|| {
        let decode_doc = docs(&[
            "Decodes and validates from wire bytes.",
            "",
            "# Errors",
            "",
            "[`DecodeError::Malformed`](crate::DecodeError) when the bytes are not",
            "protobuf, [`DecodeError::Invalid`](crate::DecodeError) when they decode",
            "but break the contract.",
        ]);
        let encode_doc = docs(&["Encodes the canonical wire form."]);
        quote! {
            #decode_doc
            pub fn decode(bytes: &[u8]) -> Result<Self, crate::DecodeError> {
                let wire = <wire::#name as ::prost::Message>::decode(bytes)
                    .map_err(crate::DecodeError::Malformed)?;
                Self::try_from(wire).map_err(crate::DecodeError::Invalid)
            }

            #encode_doc
            #[must_use]
            pub fn encode_to_vec(&self) -> Vec<u8> {
                ::prost::Message::encode_to_vec(&wire::#name::from(self.clone()))
            }
        }
    });

    // Canonical order walks fields by number — hash-visible first, metadata
    // as the tiebreak — never by declaration order.
    let mut ordered_plans: Vec<&Plan> = plans.iter().collect();
    ordered_plans.sort_by_key(|plan| (plan.metadata, plan.number));
    let cmp_chain = {
        let mut comparisons = ordered_plans.iter().map(|plan| &plan.cmp);
        comparisons.next().map_or_else(
            || quote! { std::cmp::Ordering::Equal },
            |first| {
                let rest = comparisons;
                quote! { #first #(.then_with(|| #rest))* }
            },
        )
    };
    let digests = ordered_plans
        .iter()
        .filter(|plan| !plan.metadata)
        .map(|plan| &plan.digest);
    let canonical_impls = quote! {
        impl crate::canonical::Canonical for #name {
            fn canonical_cmp(&self, other: &Self) -> std::cmp::Ordering {
                #cmp_chain
            }
        }

        impl crate::canonical::Digest for #name {
            fn digest<H: std::hash::Hasher>(&self, state: &mut H) {
                #(#digests)*
                crate::canonical::end(state);
            }
        }
    };

    // Canonicalization sorts what the schema calls `unordered`, so equal
    // content is one representation: encode emits it, hashing counts on it,
    // and the uniqueness scan reads neighbors instead of building sets.
    let sortable: Vec<&Ident> = plans
        .iter()
        .filter(|plan| {
            message
                .get_field_by_name(&plan.ident.to_string())
                .and_then(|field| vocabulary.field_invariant(&field))
                .is_some_and(|invariant| invariant.unordered)
        })
        .map(|plan| &plan.ident)
        .collect();
    let canonicalize = (!sortable.is_empty()).then(|| {
        quote! {
            fn canonicalize(&mut self) {
                #(self.#sortable.sort_by(crate::canonical::Canonical::canonical_cmp);)*
            }
        }
    });
    let canonicalize_call = canonicalize
        .is_some()
        .then(|| quote! { built.canonicalize(); });
    let built_binding = if canonicalize.is_some() {
        quote! { let mut built }
    } else {
        quote! { let built }
    };

    let content_hash = vocabulary
        .message_invariant(message)
        .is_some_and(|invariant| invariant.hashable)
        .then(|| {
            let hash_doc = docs(&[
                "Feeds this message's logical content into `state`.".to_owned(),
                String::new(),
                "Present hash-visible fields, labeled by field number; collection".to_owned(),
                "metadata is skipped, transitively. Equal content produces equal".to_owned(),
                "bytes because construction canonicalized the representation, and".to_owned(),
                "the stream is injective, so distinct content cannot collide by".to_owned(),
                "construction of the bytes alone. Encoded wire bytes are never".to_owned(),
                "fed to a hash.".to_owned(),
                String::new(),
                "The hasher is the caller's: whoever stores or compares hashes".to_owned(),
                "owns the choice of function, and the standard library's default".to_owned(),
                "is deliberately not stable across processes.".to_owned(),
            ]);
            quote! {
                #hash_doc
                pub fn content_hash<H: std::hash::Hasher>(&self, state: &mut H) {
                    crate::canonical::Digest::digest(self, state);
                }
            }
        });

    let idents: Vec<&Ident> = plans.iter().map(|plan| &plan.ident).collect();
    let decl_tys: Vec<&TokenStream> = plans.iter().map(|plan| &plan.decl_ty).collect();
    let builder_tys: Vec<&TokenStream> = plans.iter().map(|plan| &plan.builder_ty).collect();
    let setters = plans.iter().map(|plan| &plan.setter);
    let build_inits = plans.iter().map(|plan| &plan.build_init);
    let from_wires = plans.iter().map(|plan| &plan.from_wire);
    let into_wires = plans.iter().map(|plan| &plan.into_wire);
    let checks = plans.iter().map(|plan| &plan.checks);
    let accessors = plans.iter().map(|plan| &plan.accessor);

    items.extend(quote! {
        #struct_doc
        #derive
        pub struct #name {
            #(#idents: #decl_tys,)*
        }

        #canonical_impls

        impl #name {
            #builder_fn_doc
            #[must_use]
            pub fn builder() -> #builder {
                #builder::default()
            }

            #(#accessors)*

            #content_hash

            fn check(&self) -> Result<(), Invalid> {
                #(#checks)*
                rules::#rules_fn(self)?;
                Ok(())
            }

            #canonicalize

            #codec
        }

        #builder_doc
        #[derive(Clone, Debug, Default)]
        pub struct #builder {
            #(#idents: #builder_tys,)*
        }

        impl #builder {
            #(#setters)*

            #build_doc
            pub fn build(self) -> Result<#name, Invalid> {
                #built_binding = #name {
                    #(#idents: #build_inits,)*
                };
                #canonicalize_call
                built.check()?;
                Ok(built)
            }
        }

        impl TryFrom<wire::#name> for #name {
            type Error = Invalid;

            fn try_from(wire: wire::#name) -> Result<Self, Invalid> {
                #built_binding = Self {
                    #(#idents: #from_wires,)*
                };
                #canonicalize_call
                built.check()?;
                Ok(built)
            }
        }

        impl From<#name> for wire::#name {
            fn from(value: #name) -> Self {
                Self {
                    #(#idents: #into_wires,)*
                }
            }
        }
    });

    Ok(items)
}

/// Plans a oneof: returns its enum's items and the field plan the
/// containing message uses.
///
/// The shape follows the oneof's own annotation. `required` reshapes absence
/// away — the field is the enum, and a wire message without a case is
/// invalid — while a oneof the schema leaves optional stays `Option`, because
/// a validator stricter than the schema is inventing a rule, which is the
/// mirror image of missing one.
// Two plans differing in a handful of fields; splitting them apart would
// hide that they are one shape with and without absence.
#[allow(clippy::too_many_lines)]
fn plan_oneof(
    oneof: &prost_reflect::OneofDescriptor,
    vocabulary: &Vocabulary,
    claimed: &mut BTreeSet<String>,
) -> Result<(TokenStream, Plan), String> {
    let name = oneof.name();
    let id = ident(name);
    claim(
        claimed,
        &camel(name),
        &format!("oneof `{}`", oneof.full_name()),
    )?;
    let enum_name = ident(&camel(name));
    let parent = short_name(oneof.parent_message().full_name());
    let parent_module = ident(&snake(&parent));
    let required = vocabulary
        .oneof_invariant(oneof)
        .is_some_and(|invariant| invariant.required);
    // The oneof sits in the canonical field order at its members' position;
    // members share one contiguous block by construction, so the smallest
    // number stands for all of them.
    let number = oneof
        .fields()
        .map(|member| member.number())
        .min()
        .unwrap_or(u32::MAX);

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
            ident(&camel(member.name())),
            ident(&short_name(inner.full_name())),
            member.number(),
        ));
    }

    let enum_doc = if required {
        docs(&[
            format!("The `{name}` of an `nv.telemetry.v1.{parent}`: exactly one case, always"),
            "set — the oneof is `required`, so absence is unrepresentable here.".to_owned(),
        ])
    } else {
        docs(&[format!(
            "The `{name}` of an `nv.telemetry.v1.{parent}`: one case when set."
        )])
    };
    let variants = arms.iter().map(|(field_name, arm, inner, _)| {
        let doc = docs(&[format!("`{field_name}`.")]);
        quote! { #doc #arm(#inner), }
    });
    // The case is ordered and labeled by its arm's field number, exactly as a
    // field would be: two cases are compared by number first, and the digest
    // tags the payload with it, so different arms are different content.
    let cmp_arms = arms.iter().map(|(_, arm, _, _)| {
        quote! {
            (#enum_name::#arm(left), #enum_name::#arm(right)) =>
                crate::canonical::Canonical::canonical_cmp(left, right),
        }
    });
    let number_arms = arms.iter().map(|(_, arm, _, arm_number)| {
        let literal = Literal::u32_unsuffixed(*arm_number);
        quote! { #enum_name::#arm(_) => #literal, }
    });
    let digest_arms = arms.iter().map(|(_, arm, _, arm_number)| {
        let literal = Literal::u32_unsuffixed(*arm_number);
        quote! {
            #enum_name::#arm(inner) => {
                crate::canonical::tag(state, #literal);
                crate::canonical::Digest::digest(inner, state);
            }
        }
    });
    let payload_enum = quote! {
        #enum_doc
        #[derive(Clone, Debug, PartialEq, Eq)]
        #[non_exhaustive]
        pub enum #enum_name {
            #(#variants)*
        }

        impl #enum_name {
            fn arm(&self) -> u32 {
                match self {
                    #(#number_arms)*
                }
            }
        }

        impl crate::canonical::Canonical for #enum_name {
            fn canonical_cmp(&self, other: &Self) -> std::cmp::Ordering {
                match (self, other) {
                    #(#cmp_arms)*
                    _ => self.arm().cmp(&other.arm()),
                }
            }
        }

        impl crate::canonical::Digest for #enum_name {
            fn digest<H: std::hash::Hasher>(&self, state: &mut H) {
                match self {
                    #(#digest_arms)*
                }
            }
        }
    };

    let from_arms = arms.iter().map(|(field_name, arm, inner, _)| {
        quote! {
            wire::#parent_module::#enum_name::#arm(inner) => #enum_name::#arm(
                #inner::try_from(inner).map_err(|error| error.at(#field_name))?,
            ),
        }
    });
    let into_arms = arms.iter().map(|(_, arm, _, _)| {
        quote! {
            #enum_name::#arm(inner) => wire::#parent_module::#enum_name::#arm(inner.into()),
        }
    });

    let setter_doc = docs(&[format!("Sets `{name}`.")]);
    let setter = quote! {
        #setter_doc
        #[must_use]
        pub fn #id(mut self, #id: #enum_name) -> Self {
            self.#id = Some(#id);
            self
        }
    };

    let plan = if required {
        let accessor_doc = docs(&[format!("The `{name}`.")]);
        Plan {
            number,
            metadata: false,
            cmp: quote! {
                crate::canonical::Canonical::canonical_cmp(&self.#id, &other.#id)
            },
            digest: quote! {
                crate::canonical::Digest::digest(&self.#id, state);
            },
            decl_ty: quote! { #enum_name },
            builder_ty: quote! { Option<#enum_name> },
            setter,
            build_init: quote! {
                self.#id.ok_or_else(|| Invalid::field(#name, Violation::Absent))?
            },
            from_wire: quote! {
                match wire.#id.ok_or_else(|| Invalid::field(#name, Violation::Absent))? {
                    #(#from_arms)*
                }
            },
            into_wire: quote! {
                Some(match value.#id {
                    #(#into_arms)*
                })
            },
            checks: TokenStream::new(),
            accessor: quote! {
                #accessor_doc
                #[must_use]
                pub fn #id(&self) -> &#enum_name {
                    &self.#id
                }
            },
            ident: id,
        }
    } else {
        let accessor_doc = docs(&[format!("The `{name}`, when present.")]);
        Plan {
            number,
            metadata: false,
            cmp: quote! {
                crate::canonical::cmp_option(self.#id.as_ref(), other.#id.as_ref())
            },
            digest: quote! {
                if let Some(case) = &self.#id {
                    crate::canonical::Digest::digest(case, state);
                }
            },
            decl_ty: quote! { Option<#enum_name> },
            builder_ty: quote! { Option<#enum_name> },
            setter,
            build_init: quote! { self.#id },
            from_wire: quote! {
                match wire.#id {
                    None => None,
                    Some(case) => Some(match case {
                        #(#from_arms)*
                    }),
                }
            },
            into_wire: quote! {
                value.#id.map(|case| match case {
                    #(#into_arms)*
                })
            },
            checks: TokenStream::new(),
            accessor: quote! {
                #accessor_doc
                #[must_use]
                pub fn #id(&self) -> Option<&#enum_name> {
                    self.#id.as_ref()
                }
            },
            ident: id,
        }
    };

    Ok((payload_enum, plan))
}

/// Plans one regular field.
// One match arm per field category, and the arms are what the function is:
// splitting each into its own function would hide that the categories differ
// only in the tokens they produce.
#[allow(clippy::too_many_lines)]
fn plan_field(field: &FieldDescriptor, vocabulary: &Vocabulary) -> Result<Plan, String> {
    let invariant = vocabulary.field_invariant(field).unwrap_or_default();
    let name = field.name().to_owned();
    let id = ident(&name);
    let lit = name.as_str();
    let number = field.number();
    let tag = Literal::u32_unsuffixed(number);
    let metadata = invariant.collection_metadata;

    let absent = quote! { .ok_or_else(|| Invalid::field(#lit, Violation::Absent))? };

    let max_len = invariant
        .max_len
        .map(|_| ident(&format!("{}_MAX_LEN", constant_stem(field.full_name()))));
    let max_items = invariant
        .max_items
        .map(|_| ident(&format!("{}_MAX_ITEMS", constant_stem(field.full_name()))));

    let mut checks = TokenStream::new();

    let plan = match field.kind() {
        Kind::String if field.is_list() => {
            if let Some(limit) = &max_items {
                checks.extend(quote! {
                    if let Some(violation) = invalid::too_many(self.#id.len(), limits::#limit) {
                        return Err(Invalid::field(#lit, violation));
                    }
                });
            }
            if invariant.non_empty || max_len.is_some() {
                let empty = invariant.non_empty.then(|| {
                    quote! {
                        if element.is_empty() {
                            return Err(Invalid::element(#lit, index, Violation::Empty));
                        }
                    }
                });
                let long = max_len.as_ref().map(|limit| {
                    quote! {
                        if let Some(violation) = invalid::too_long(element.len(), limits::#limit) {
                            return Err(Invalid::element(#lit, index, violation));
                        }
                    }
                });
                checks.extend(quote! {
                    for (index, element) in self.#id.iter().enumerate() {
                        #empty
                        #long
                    }
                });
            }
            let setter_doc = docs(&[format!("Sets `{name}`.")]);
            let accessor_doc = docs(&[format!("The `{name}`.")]);
            Plan {
                number,
                metadata,
                cmp: quote! { crate::canonical::cmp_slice(&self.#id, &other.#id) },
                digest: quote! {
                    crate::canonical::tag(state, #tag);
                    crate::canonical::count(state, self.#id.len());
                    for element in &self.#id {
                        crate::canonical::str_value(state, element);
                    }
                },
                decl_ty: quote! { Vec<String> },
                builder_ty: quote! { Vec<String> },
                setter: quote! {
                    #setter_doc
                    #[must_use]
                    pub fn #id(mut self, #id: Vec<String>) -> Self {
                        self.#id = #id;
                        self
                    }
                },
                build_init: quote! { self.#id },
                from_wire: quote! { wire.#id },
                into_wire: quote! { value.#id },
                accessor: quote! {
                    #accessor_doc
                    #[must_use]
                    pub fn #id(&self) -> &[String] {
                        &self.#id
                    }
                },
                checks,
                ident: id,
            }
        }
        Kind::String => {
            let required = invariant.required;
            if invariant.non_empty {
                let inner = quote! {
                    if element.is_empty() {
                        return Err(Invalid::field(#lit, Violation::Empty));
                    }
                };
                checks.extend(if required {
                    quote! {
                        if self.#id.is_empty() {
                            return Err(Invalid::field(#lit, Violation::Empty));
                        }
                    }
                } else {
                    quote! { if let Some(element) = &self.#id { #inner } }
                });
            }
            if let Some(limit) = &max_len {
                checks.extend(if required {
                    quote! {
                        if let Some(violation) = invalid::too_long(self.#id.len(), limits::#limit) {
                            return Err(Invalid::field(#lit, violation));
                        }
                    }
                } else {
                    quote! {
                        if let Some(element) = &self.#id {
                            if let Some(violation) = invalid::too_long(element.len(), limits::#limit) {
                                return Err(Invalid::field(#lit, violation));
                            }
                        }
                    }
                });
            }
            let setter_doc = docs(&[format!("Sets `{name}`.")]);
            let setter = quote! {
                #setter_doc
                #[must_use]
                pub fn #id(mut self, #id: impl Into<String>) -> Self {
                    self.#id = Some(#id.into());
                    self
                }
            };
            if required {
                let accessor_doc = docs(&[format!("The `{name}`.")]);
                Plan {
                    number,
                    metadata,
                    cmp: quote! { self.#id.cmp(&other.#id) },
                    digest: quote! {
                        crate::canonical::tag(state, #tag);
                        crate::canonical::str_value(state, &self.#id);
                    },
                    decl_ty: quote! { String },
                    builder_ty: quote! { Option<String> },
                    setter,
                    build_init: quote! { self.#id #absent },
                    from_wire: quote! { wire.#id #absent },
                    into_wire: quote! { Some(value.#id) },
                    accessor: quote! {
                        #accessor_doc
                        #[must_use]
                        pub fn #id(&self) -> &str {
                            &self.#id
                        }
                    },
                    checks,
                    ident: id,
                }
            } else {
                let accessor_doc = docs(&[format!("The `{name}`, when present.")]);
                Plan {
                    number,
                    metadata,
                    cmp: quote! {
                        crate::canonical::cmp_option(self.#id.as_ref(), other.#id.as_ref())
                    },
                    digest: quote! {
                        if let Some(element) = &self.#id {
                            crate::canonical::tag(state, #tag);
                            crate::canonical::str_value(state, element);
                        }
                    },
                    decl_ty: quote! { Option<String> },
                    builder_ty: quote! { Option<String> },
                    setter,
                    build_init: quote! { self.#id },
                    from_wire: quote! { wire.#id },
                    into_wire: quote! { value.#id },
                    accessor: quote! {
                        #accessor_doc
                        #[must_use]
                        pub fn #id(&self) -> Option<&str> {
                            self.#id.as_deref()
                        }
                    },
                    checks,
                    ident: id,
                }
            }
        }
        Kind::Bool => copy_plan(
            &invariant,
            field,
            id,
            lit,
            &quote! { bool },
            &absent,
            "bool_value",
        ),
        Kind::Uint64 => {
            if invariant.required {
                return Err(format!(
                    "`{}`: a required bare integer needs a reshaping decision \
                     this generator has not made yet",
                    field.full_name()
                ));
            }
            copy_plan(
                &invariant,
                field,
                id,
                lit,
                &quote! { u64 },
                &absent,
                "u64_value",
            )
        }
        Kind::Enum(declared) => {
            let ty = ident(&short_name(declared.full_name()));
            let setter_doc = docs(&[format!("Sets `{name}`.")]);
            let setter = quote! {
                #setter_doc
                #[must_use]
                pub fn #id(mut self, #id: #ty) -> Self {
                    self.#id = Some(#id);
                    self
                }
            };
            if invariant.required {
                let accessor_doc = docs(&[format!("The `{name}`.")]);
                Plan {
                    number,
                    metadata,
                    cmp: quote! { i32::from(self.#id).cmp(&i32::from(other.#id)) },
                    digest: quote! {
                        crate::canonical::tag(state, #tag);
                        crate::canonical::i32_value(state, i32::from(self.#id));
                    },
                    decl_ty: quote! { #ty },
                    builder_ty: quote! { Option<#ty> },
                    setter,
                    build_init: quote! { self.#id #absent },
                    from_wire: quote! {
                        #ty::try_from(wire.#id #absent)
                            .map_err(|violation| Invalid::field(#lit, violation))?
                    },
                    into_wire: quote! { Some(value.#id.into()) },
                    accessor: quote! {
                        #accessor_doc
                        #[must_use]
                        pub fn #id(&self) -> #ty {
                            self.#id
                        }
                    },
                    checks,
                    ident: id,
                }
            } else {
                let accessor_doc = docs(&[format!("The `{name}`, when present.")]);
                Plan {
                    number,
                    metadata,
                    cmp: quote! {
                        crate::canonical::cmp_option(self.#id.as_ref(), other.#id.as_ref())
                    },
                    digest: quote! {
                        if let Some(value) = self.#id {
                            crate::canonical::tag(state, #tag);
                            crate::canonical::i32_value(state, i32::from(value));
                        }
                    },
                    decl_ty: quote! { Option<#ty> },
                    builder_ty: quote! { Option<#ty> },
                    setter,
                    build_init: quote! { self.#id },
                    from_wire: quote! {
                        wire.#id
                            .map(#ty::try_from)
                            .transpose()
                            .map_err(|violation| Invalid::field(#lit, violation))?
                    },
                    into_wire: quote! { value.#id.map(Into::into) },
                    accessor: quote! {
                        #accessor_doc
                        #[must_use]
                        pub fn #id(&self) -> Option<#ty> {
                            self.#id
                        }
                    },
                    checks,
                    ident: id,
                }
            }
        }
        Kind::Message(inner) if inner.full_name() == "nv.telemetry.v1.Value.Map" => {
            checks.extend(quote! {
                if let Some(map) = &self.#id {
                    value::check_map(map, #lit)?;
                }
            });
            let setter_doc = docs(&[format!("Sets `{name}`.")]);
            let accessor_doc = docs(&[format!("The `{name}`, when present.")]);
            Plan {
                number,
                metadata,
                cmp: quote! {
                    crate::canonical::cmp_option_map(self.#id.as_ref(), other.#id.as_ref())
                },
                digest: quote! {
                    if let Some(map) = &self.#id {
                        crate::canonical::tag(state, #tag);
                        crate::canonical::map_value(state, map);
                    }
                },
                decl_ty: quote! { Option<BTreeMap<String, Value>> },
                builder_ty: quote! { Option<BTreeMap<String, Value>> },
                setter: quote! {
                    #setter_doc
                    #[must_use]
                    pub fn #id(mut self, #id: BTreeMap<String, Value>) -> Self {
                        self.#id = Some(#id);
                        self
                    }
                },
                build_init: quote! { self.#id },
                from_wire: quote! {
                    wire.#id
                        .map(value::map_from_wire)
                        .transpose()
                        .map_err(|error| error.at(#lit))?
                },
                into_wire: quote! { value.#id.map(value::map_into_wire) },
                accessor: quote! {
                    #accessor_doc
                    #[must_use]
                    pub fn #id(&self) -> Option<&BTreeMap<String, Value>> {
                        self.#id.as_ref()
                    }
                },
                checks,
                ident: id,
            }
        }
        Kind::Message(inner) => {
            let ty = ident(
                hand_written_type(inner.full_name())
                    .map_or_else(|| short_name(inner.full_name()), ToOwned::to_owned)
                    .as_str(),
            );
            if field.is_list() {
                if let Some(limit) = &max_items {
                    checks.extend(quote! {
                        if let Some(violation) = invalid::too_many(self.#id.len(), limits::#limit) {
                            return Err(Invalid::field(#lit, violation));
                        }
                    });
                }
                if !invariant.unique_by.is_empty() {
                    let keys: Vec<Ident> =
                        invariant.unique_by.iter().map(|key| ident(key)).collect();
                    if adjacent_scan_sound(&inner, &invariant, vocabulary) {
                        // Canonicalization has sorted the elements, and the
                        // keys are the most significant fields of that order,
                        // so equal keys are neighbors: one pass, no set, no
                        // per-probe string comparisons — the cost the benches
                        // indicted.
                        let first = &keys[0];
                        let rest = &keys[1..];
                        checks.extend(quote! {
                            for index in 1..self.#id.len() {
                                if self.#id[index].#first == self.#id[index - 1].#first
                                    #(&& self.#id[index].#rest == self.#id[index - 1].#rest)*
                                {
                                    return Err(Invalid::element(#lit, index, Violation::Duplicate));
                                }
                            }
                        });
                    } else {
                        // The keys are not the leading fields of the canonical
                        // order (or the field is not sorted at all), so equal
                        // keys need not be adjacent and a set does the work.
                        let key_expr = if keys.len() == 1 {
                            let key = &keys[0];
                            quote! { &element.#key }
                        } else {
                            quote! { (#(&element.#keys),*) }
                        };
                        checks.extend(quote! {
                            let mut seen = BTreeSet::new();
                            for (index, element) in self.#id.iter().enumerate() {
                                if !seen.insert(#key_expr) {
                                    return Err(Invalid::element(#lit, index, Violation::Duplicate));
                                }
                            }
                        });
                    }
                }
                let setter_doc = docs(&[format!("Sets `{name}`.")]);
                let accessor_doc = docs(&[format!("The `{name}`.")]);
                Plan {
                    number,
                    metadata,
                    cmp: quote! { crate::canonical::cmp_slice(&self.#id, &other.#id) },
                    digest: quote! {
                        crate::canonical::tag(state, #tag);
                        crate::canonical::count(state, self.#id.len());
                        for element in &self.#id {
                            crate::canonical::Digest::digest(element, state);
                        }
                    },
                    decl_ty: quote! { Vec<#ty> },
                    builder_ty: quote! { Vec<#ty> },
                    setter: quote! {
                        #setter_doc
                        #[must_use]
                        pub fn #id(mut self, #id: Vec<#ty>) -> Self {
                            self.#id = #id;
                            self
                        }
                    },
                    build_init: quote! { self.#id },
                    from_wire: quote! {
                        {
                            let mut elements = Vec::with_capacity(wire.#id.len());
                            for (index, element) in wire.#id.into_iter().enumerate() {
                                elements.push(
                                    #ty::try_from(element)
                                        .map_err(|error| error.at_index(#lit, index))?,
                                );
                            }
                            elements
                        }
                    },
                    into_wire: quote! { value.#id.into_iter().map(Into::into).collect() },
                    accessor: quote! {
                        #accessor_doc
                        #[must_use]
                        pub fn #id(&self) -> &[#ty] {
                            &self.#id
                        }
                    },
                    checks,
                    ident: id,
                }
            } else if invariant.required {
                let setter_doc = docs(&[format!("Sets `{name}`.")]);
                let accessor_doc = docs(&[format!("The `{name}`.")]);
                Plan {
                    number,
                    metadata,
                    cmp: quote! {
                        crate::canonical::Canonical::canonical_cmp(&self.#id, &other.#id)
                    },
                    digest: quote! {
                        crate::canonical::tag(state, #tag);
                        crate::canonical::Digest::digest(&self.#id, state);
                    },
                    decl_ty: quote! { #ty },
                    builder_ty: quote! { Option<#ty> },
                    setter: quote! {
                        #setter_doc
                        #[must_use]
                        pub fn #id(mut self, #id: #ty) -> Self {
                            self.#id = Some(#id);
                            self
                        }
                    },
                    build_init: quote! { self.#id #absent },
                    from_wire: quote! {
                        #ty::try_from(wire.#id #absent)
                            .map_err(|error| error.at(#lit))?
                    },
                    into_wire: quote! { Some(value.#id.into()) },
                    accessor: quote! {
                        #accessor_doc
                        #[must_use]
                        pub fn #id(&self) -> &#ty {
                            &self.#id
                        }
                    },
                    checks,
                    ident: id,
                }
            } else {
                let setter_doc = docs(&[format!("Sets `{name}`.")]);
                let accessor_doc = docs(&[format!("The `{name}`, when present.")]);
                Plan {
                    number,
                    metadata,
                    cmp: quote! {
                        crate::canonical::cmp_option(self.#id.as_ref(), other.#id.as_ref())
                    },
                    digest: quote! {
                        if let Some(element) = &self.#id {
                            crate::canonical::tag(state, #tag);
                            crate::canonical::Digest::digest(element, state);
                        }
                    },
                    decl_ty: quote! { Option<#ty> },
                    builder_ty: quote! { Option<#ty> },
                    setter: quote! {
                        #setter_doc
                        #[must_use]
                        pub fn #id(mut self, #id: #ty) -> Self {
                            self.#id = Some(#id);
                            self
                        }
                    },
                    build_init: quote! { self.#id },
                    from_wire: quote! {
                        wire.#id
                            .map(#ty::try_from)
                            .transpose()
                            .map_err(|error| error.at(#lit))?
                    },
                    into_wire: quote! { value.#id.map(Into::into) },
                    accessor: quote! {
                        #accessor_doc
                        #[must_use]
                        pub fn #id(&self) -> Option<&#ty> {
                            self.#id.as_ref()
                        }
                    },
                    checks,
                    ident: id,
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

/// Whether a sorted adjacent scan can replace the uniqueness set: the field
/// must be `unordered` — canonicalization only sorts those — and its keys
/// must be exactly the leading hash-visible field numbers of the element, so
/// the canonical order groups equal keys together.
fn adjacent_scan_sound(
    element: &MessageDescriptor,
    invariant: &FieldInvariant,
    vocabulary: &Vocabulary,
) -> bool {
    if !invariant.unordered {
        return false;
    }
    let mut visible: Vec<u32> = element
        .fields()
        .filter(|member| {
            !vocabulary
                .field_invariant(member)
                .is_some_and(|member| member.collection_metadata)
        })
        .map(|member| member.number())
        .collect();
    visible.sort_unstable();

    let mut keys: Vec<u32> = invariant
        .unique_by
        .iter()
        .filter_map(|key| element.get_field_by_name(key))
        .map(|member| member.number())
        .collect();
    keys.sort_unstable();

    visible.len() >= keys.len() && visible[..keys.len()] == keys[..]
}

/// The plan for a `Copy` scalar — `bool`, `u64` — where required and optional
/// differ only in the declared type and the absence check.
fn copy_plan(
    invariant: &FieldInvariant,
    field: &FieldDescriptor,
    id: Ident,
    lit: &str,
    ty: &TokenStream,
    absent: &TokenStream,
    writer: &str,
) -> Plan {
    let name = lit;
    let number = field.number();
    let tag = Literal::u32_unsuffixed(number);
    let write = ident(writer);
    let setter_doc = docs(&[format!("Sets `{name}`.")]);
    let setter = quote! {
        #setter_doc
        #[must_use]
        pub fn #id(mut self, #id: #ty) -> Self {
            self.#id = Some(#id);
            self
        }
    };
    if invariant.required {
        let accessor_doc = docs(&[format!("The `{name}`.")]);
        Plan {
            number,
            metadata: invariant.collection_metadata,
            cmp: quote! { self.#id.cmp(&other.#id) },
            digest: quote! {
                crate::canonical::tag(state, #tag);
                crate::canonical::#write(state, self.#id);
            },
            decl_ty: quote! { #ty },
            builder_ty: quote! { Option<#ty> },
            setter,
            build_init: quote! { self.#id #absent },
            from_wire: quote! { wire.#id #absent },
            into_wire: quote! { Some(value.#id) },
            accessor: quote! {
                #accessor_doc
                #[must_use]
                pub fn #id(&self) -> #ty {
                    self.#id
                }
            },
            checks: TokenStream::new(),
            ident: id,
        }
    } else {
        let accessor_doc = docs(&[format!("The `{name}`, when present.")]);
        Plan {
            number,
            metadata: invariant.collection_metadata,
            cmp: quote! {
                crate::canonical::cmp_option(self.#id.as_ref(), other.#id.as_ref())
            },
            digest: quote! {
                if let Some(value) = self.#id {
                    crate::canonical::tag(state, #tag);
                    crate::canonical::#write(state, value);
                }
            },
            decl_ty: quote! { Option<#ty> },
            builder_ty: quote! { Option<#ty> },
            setter,
            build_init: quote! { self.#id },
            from_wire: quote! { wire.#id },
            into_wire: quote! { value.#id },
            accessor: quote! {
                #accessor_doc
                #[must_use]
                pub fn #id(&self) -> Option<#ty> {
                    self.#id
                }
            },
            checks: TokenStream::new(),
            ident: id,
        }
    }
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
