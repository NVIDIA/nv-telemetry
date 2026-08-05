// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The `limits` module emitter: one constant per bound the schema declares.

use std::fmt::Write as _;

use prost_reflect::DescriptorPool;

use super::names::constant_stem;
use super::names::separated;
use crate::is_contract_package;
use crate::options::Vocabulary;

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
