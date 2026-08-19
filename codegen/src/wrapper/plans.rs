// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Field and oneof plans: everything the generator needs to know about one
//! declaration, computed once, consumed by the message assembly in the
//! parent module.

use std::collections::BTreeSet;

use proc_macro2::Ident;
use proc_macro2::Literal;
use proc_macro2::TokenStream;
use prost_reflect::FieldDescriptor;
use prost_reflect::Kind;
use prost_reflect::MessageDescriptor;
use quote::quote;

use super::claim;
use super::hand_written_type;
use super::names::camel;
use super::names::constant_stem;
use super::names::docs;
use super::names::ident;
use super::names::short_name;
use super::names::snake;
use crate::options::FieldInvariant;
use crate::options::Vocabulary;

/// Everything the generator needs to know about one field, computed once.
pub(super) struct Plan {
    /// Field name as an identifier; also names the accessor and setter.
    pub(super) ident: Ident,
    /// Field number: the canonical order compares and the digest labels by
    /// it, never by declaration order.
    pub(super) number: u32,
    /// Collection metadata: skipped by the digest, compared only in the
    /// canonical order's tiebreak phase.
    pub(super) metadata: bool,
    /// Sorted by canonicalization: an `unordered` repeated field. Recorded
    /// when the plan is made, where the invariant is in hand — re-deriving it
    /// later by field name would turn any name mismatch into a field that
    /// silently stops being sorted, which unsounds the adjacent scan.
    pub(super) sortable: bool,
    /// Comparison expression against `other` for the canonical order.
    pub(super) cmp: TokenStream,
    /// Statements feeding this field to the digest.
    pub(super) digest: TokenStream,
    /// Statements emitting this field's wire bytes.
    pub(super) emit: TokenStream,
    /// Expression for this field's encoded length.
    pub(super) emit_len: TokenStream,
    /// Declared type in the validated struct.
    pub(super) decl_ty: TokenStream,
    /// Declared type in the builder.
    pub(super) builder_ty: TokenStream,
    /// The builder's setter method.
    pub(super) setter: TokenStream,
    /// Expression building the validated field inside `build`, consuming
    /// `self.<name>`.
    pub(super) build_init: TokenStream,
    /// Expression building the validated field inside `TryFrom`, consuming
    /// `wire.<name>`.
    pub(super) from_wire: TokenStream,
    /// Expression rebuilding the wire field, consuming `value.<name>`.
    pub(super) into_wire: TokenStream,
    /// Statements for `check`, referencing `self.<name>`.
    pub(super) checks: TokenStream,
    /// The accessor method.
    pub(super) accessor: TokenStream,
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
pub(super) fn plan_oneof(
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
    let emit_arms = arms.iter().map(|(_, arm, _, arm_number)| {
        let literal = Literal::u32_unsuffixed(*arm_number);
        quote! { #enum_name::#arm(inner) => crate::encode::nested(#literal, inner, buf), }
    });
    let emit_len_arms = arms.iter().map(|(_, arm, _, arm_number)| {
        let literal = Literal::u32_unsuffixed(*arm_number);
        quote! { #enum_name::#arm(inner) => crate::encode::nested_len(#literal, inner), }
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

            fn emit(&self, buf: &mut impl ::prost::bytes::BufMut) {
                match self {
                    #(#emit_arms)*
                }
            }

            fn emitted_len(&self) -> usize {
                match self {
                    #(#emit_len_arms)*
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
            sortable: false,
            cmp: quote! {
                crate::canonical::Canonical::canonical_cmp(&self.#id, &other.#id)
            },
            digest: quote! {
                crate::canonical::Digest::digest(&self.#id, state);
            },
            emit: quote! { self.#id.emit(buf); },
            emit_len: quote! { self.#id.emitted_len() },
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
            sortable: false,
            cmp: quote! {
                crate::canonical::cmp_option(self.#id.as_ref(), other.#id.as_ref())
            },
            digest: quote! {
                if let Some(case) = &self.#id {
                    crate::canonical::Digest::digest(case, state);
                }
            },
            emit: quote! {
                if let Some(case) = &self.#id {
                    case.emit(buf);
                }
            },
            emit_len: quote! {
                self.#id.as_ref().map_or(0, |case| case.emitted_len())
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
pub(super) fn plan_field(field: &FieldDescriptor, vocabulary: &Vocabulary) -> Result<Plan, String> {
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
                sortable: invariant.unordered,
                cmp: quote! { crate::canonical::cmp_slice(&self.#id, &other.#id) },
                digest: quote! {
                    crate::canonical::tag(state, #tag);
                    crate::canonical::count(state, self.#id.len());
                    for element in &self.#id {
                        crate::canonical::str_value(state, element);
                    }
                },
                emit: quote! { ::prost::encoding::string::encode_repeated(#tag, &self.#id, buf); },
                emit_len: quote! { ::prost::encoding::string::encoded_len_repeated(#tag, &self.#id) },
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
                    sortable: false,
                    cmp: quote! { self.#id.cmp(&other.#id) },
                    digest: quote! {
                        crate::canonical::tag(state, #tag);
                        crate::canonical::str_value(state, &self.#id);
                    },
                    emit: quote! { ::prost::encoding::string::encode(#tag, &self.#id, buf); },
                    emit_len: quote! { ::prost::encoding::string::encoded_len(#tag, &self.#id) },
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
                    sortable: false,
                    cmp: quote! {
                        crate::canonical::cmp_option(self.#id.as_ref(), other.#id.as_ref())
                    },
                    digest: quote! {
                        if let Some(element) = &self.#id {
                            crate::canonical::tag(state, #tag);
                            crate::canonical::str_value(state, element);
                        }
                    },
                    emit: quote! { if let Some(element) = &self.#id { ::prost::encoding::string::encode(#tag, element, buf); } },
                    emit_len: quote! { self.#id.as_ref().map_or(0, |element| ::prost::encoding::string::encoded_len(#tag, element)) },
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
        Kind::Enum(_) if field.is_list() => {
            return Err(format!(
                "`{}`: a repeated enum needs a reshaping decision this \
                 generator has not made yet",
                field.full_name()
            ));
        }
        Kind::Enum(declared) => {
            let ty = ident(&short_name(declared.full_name()));
            // The builder accepts an already-typed enum, so a hand-built
            // `Unrecognized` aliasing a recognized discriminant arrives here
            // without passing `TryFrom<i32>`; the enum's own check refuses
            // it. Decode cannot carry such a value — `TryFrom<i32>`
            // normalizes — so there the shared check is inert uniformity.
            checks.extend(if invariant.required {
                quote! {
                    self.#id
                        .check()
                        .map_err(|violation| Invalid::field(#lit, violation))?;
                }
            } else {
                quote! {
                    if let Some(value) = self.#id {
                        value
                            .check()
                            .map_err(|violation| Invalid::field(#lit, violation))?;
                    }
                }
            });
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
                    sortable: false,
                    cmp: quote! { i32::from(self.#id).cmp(&i32::from(other.#id)) },
                    digest: quote! {
                        crate::canonical::tag(state, #tag);
                        crate::canonical::i32_value(state, i32::from(self.#id));
                    },
                    emit: quote! { ::prost::encoding::int32::encode(#tag, &i32::from(self.#id), buf); },
                    emit_len: quote! { ::prost::encoding::int32::encoded_len(#tag, &i32::from(self.#id)) },
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
                    sortable: false,
                    cmp: quote! {
                        crate::canonical::cmp_option(self.#id.as_ref(), other.#id.as_ref())
                    },
                    digest: quote! {
                        if let Some(value) = self.#id {
                            crate::canonical::tag(state, #tag);
                            crate::canonical::i32_value(state, i32::from(value));
                        }
                    },
                    emit: quote! { if let Some(value) = self.#id { ::prost::encoding::int32::encode(#tag, &i32::from(value), buf); } },
                    emit_len: quote! { self.#id.map_or(0, |value| ::prost::encoding::int32::encoded_len(#tag, &i32::from(value))) },
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
                sortable: false,
                cmp: quote! {
                    crate::canonical::cmp_option_map(self.#id.as_ref(), other.#id.as_ref())
                },
                digest: quote! {
                    if let Some(map) = &self.#id {
                        crate::canonical::tag(state, #tag);
                        crate::canonical::map_value(state, map);
                    }
                },
                emit: quote! { if let Some(map) = &self.#id { crate::encode::map_field(#tag, map, buf); } },
                emit_len: quote! { self.#id.as_ref().map_or(0, |map| crate::encode::map_field_len(#tag, map)) },
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
                    sortable: invariant.unordered,
                    cmp: quote! { crate::canonical::cmp_slice(&self.#id, &other.#id) },
                    digest: quote! {
                        crate::canonical::tag(state, #tag);
                        crate::canonical::count(state, self.#id.len());
                        for element in &self.#id {
                            crate::canonical::Digest::digest(element, state);
                        }
                    },
                    emit: quote! { for element in &self.#id { crate::encode::nested(#tag, element, buf); } },
                    emit_len: quote! { self.#id.iter().map(|element| crate::encode::nested_len(#tag, element)).sum::<usize>() },
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
                    sortable: false,
                    cmp: quote! {
                        crate::canonical::Canonical::canonical_cmp(&self.#id, &other.#id)
                    },
                    digest: quote! {
                        crate::canonical::tag(state, #tag);
                        crate::canonical::Digest::digest(&self.#id, state);
                    },
                    emit: quote! { crate::encode::nested(#tag, &self.#id, buf); },
                    emit_len: quote! { crate::encode::nested_len(#tag, &self.#id) },
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
                    sortable: false,
                    cmp: quote! {
                        crate::canonical::cmp_option(self.#id.as_ref(), other.#id.as_ref())
                    },
                    digest: quote! {
                        if let Some(element) = &self.#id {
                            crate::canonical::tag(state, #tag);
                            crate::canonical::Digest::digest(element, state);
                        }
                    },
                    emit: quote! { if let Some(element) = &self.#id { crate::encode::nested(#tag, element, buf); } },
                    emit_len: quote! { self.#id.as_ref().map_or(0, |element| crate::encode::nested_len(#tag, element)) },
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
    // One concept, two vocabularies: the digest writer implies the prost
    // encoding module for the same scalar.
    let wire_mod = ident(match writer {
        "bool_value" => "bool",
        "u64_value" => "uint64",
        other => other,
    });
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
            sortable: false,
            cmp: quote! { self.#id.cmp(&other.#id) },
            digest: quote! {
                crate::canonical::tag(state, #tag);
                crate::canonical::#write(state, self.#id);
            },
            emit: quote! { ::prost::encoding::#wire_mod::encode(#tag, &self.#id, buf); },
            emit_len: quote! { ::prost::encoding::#wire_mod::encoded_len(#tag, &self.#id) },
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
            sortable: false,
            cmp: quote! {
                crate::canonical::cmp_option(self.#id.as_ref(), other.#id.as_ref())
            },
            digest: quote! {
                if let Some(value) = self.#id {
                    crate::canonical::tag(state, #tag);
                    crate::canonical::#write(state, value);
                }
            },
            emit: quote! {
                if let Some(value) = &self.#id {
                    ::prost::encoding::#wire_mod::encode(#tag, value, buf);
                }
            },
            emit_len: quote! {
                self.#id
                    .as_ref()
                    .map_or(0, |value| ::prost::encoding::#wire_mod::encoded_len(#tag, value))
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
