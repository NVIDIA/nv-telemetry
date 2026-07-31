// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Wire types: the prost structs the contract decodes into.
//!
//! Generated from the descriptor set the schema crate already builds, by
//! calling prost-build's renderer directly rather than its compile entry
//! points. Nothing here invokes `protoc`, and the wire types cannot drift from
//! the descriptors the invariant rules ran over — they are the same bytes.
//!
//! Only the contract package is emitted. The annotation and manifest packages
//! are build-time inputs; generating them would put the compiler's own
//! vocabulary, and the canary, into a crate consumers depend on.
//!
//! These are the raw types, with public fields and no invariants. They are not
//! the public face of the data plane: the validated wrappers take the public
//! names, and these carry the `wire` module path to keep the distinction
//! legible at the use site.

use std::collections::HashMap;

use prost_build::Module;
use prost_reflect::DescriptorPool;
use prost_types::FileDescriptorProto;

use crate::is_contract_package;

/// Header for the generated module.
///
/// Deliberately short. Every entry here was measured to fire: `pedantic` on
/// prost's own `as_str_name` boilerplate, and `exhaustive_structs` on the
/// message structs. A broader block — `clippy::all` in particular — would
/// silence the deny-by-default correctness group for this file forever, and
/// generated code is exactly where nobody would notice.
const HEADER: &str = "\
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Generated from `nv.telemetry.v1` by `make codegen`. Do not edit.
//!
//! Wire types only. Every field is public and no invariant is enforced here;
//! construction that must be valid goes through the validated wrappers.

// A wire type's shape *is* the wire format, so it is exhaustive on purpose,
// and its fields are public because that is what decoding produces.
//
// The doc comments are the schema's own prose, carried through verbatim. It
// is written for a proto audience and cannot be held to rustdoc's markdown
// rules: a `<Chassis>` in a URI example is an unclosed HTML tag, and under
// `-D warnings` that would fail the build on the most innocuous edit anyone
// can make to a schema.
#![allow(clippy::pedantic, clippy::exhaustive_structs, rustdoc::all)]
";

/// Renders the contract package as Rust.
///
/// # Errors
///
/// Returns a description if prost-build cannot render the descriptors, if the
/// contract declares a sub-package this generator would silently drop, or if
/// the rendered module is not self-contained.
pub fn generate(pool: &DescriptorPool) -> Result<String, String> {
    let files: Vec<(Module, FileDescriptorProto)> = pool
        .files()
        .filter(|file| is_contract_package(file.package_name()))
        .map(|file| {
            (
                Module::from_protobuf_package_name(file.package_name()),
                file.file_descriptor_proto().clone(),
            )
        })
        .collect();

    let mut config = prost_build::Config::new();
    // Comments come through, so the schema's reasoning reaches the Rust a
    // consumer reads. They exist only because the schema crate builds its
    // descriptor set with source info retained.
    //
    // prost-build's `cleanup-markdown` feature is what makes that safe: an
    // indented example in a proto comment would otherwise become an executed
    // Rust doctest, and fail. It rewrites such blocks to `text` fences.
    config.disable_comments::<[String; 0], String>([]);
    config.format(true);

    let rendered: HashMap<Module, String> = config
        .generate(files)
        .map_err(|error| format!("wire types could not be generated: {error}"))?;

    // The file filter matches the contract package *and its sub-packages*,
    // but only one module is emitted below. A sub-package would be linted,
    // locked, and guarded by the compatibility check while having no type in
    // the data plane at all, and nothing downstream would fail. Refuse
    // instead: extending this to nested modules is a real change, and it
    // should be made deliberately rather than discovered.
    let module = Module::from_protobuf_package_name(crate::CONTRACT_PACKAGE);
    let mut extra: Vec<String> = rendered
        .keys()
        .filter(|candidate| **candidate != module)
        .map(ToString::to_string)
        .collect();
    extra.sort();
    if !extra.is_empty() {
        return Err(format!(
            "the contract declares sub-package(s) [{}] that this generator does not emit; \
             wire types support a single module",
            extra.join(", ")
        ));
    }

    let body = rendered
        .get(&module)
        .ok_or_else(|| format!("no output for `{}`", crate::CONTRACT_PACKAGE))?;

    // Belt and braces for the case the invariant rules cannot see: a type from
    // another package renders as a path out of this module, which either fails
    // to compile or, worse, resolves once an unrelated dependency is added.
    if body.contains("::prost_types::") {
        return Err(
            "the rendered wire types reference `prost_types`, so the contract has a \
             google.protobuf field the invariant rules did not reject"
                .to_owned(),
        );
    }

    Ok(format!("{HEADER}\n{body}"))
}
