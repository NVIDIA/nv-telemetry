// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Projection generation.
//!
//! Compiles manifests into extraction code, resolving paths against a
//! backend-neutral schema index. Manifests under `sources/*/manifests/` are
//! loaded ([`spec`]), resolved against the CSDL index ([`index`]) and the
//! contract pool, checked — every declaration the compiler cannot honor is
//! an error ([`lint`]) — and the checked manifests become the source
//! crate's generated projection modules ([`mod@emit`]).

pub mod compile;
pub mod emit;
pub mod index;
pub mod lint;
pub mod spec;

mod location;

pub use compile::compile;
pub use compile::CompiledManifests;
pub use emit::emit;
pub use index::Bundle;
pub use index::IndexError;
pub use index::RedfishIndex;
pub use index::ResolvedField;
pub use index::Shape;
pub use index::Step;
pub use lint::Violation;
pub use spec::load;
pub use spec::ManifestError;
pub use spec::ManifestSpec;
pub use spec::WorkspaceRelativePath;
pub use spec::WorkspaceRelativePathError;

/// Runs the complete projection compiler as a diagnostic-only facade.
///
/// Unlike the internal surface lint, this includes typed lowering: a
/// conversion or derived name that cannot become a plan is a violation here,
/// not a later emitter error.
#[must_use]
pub fn check(
    manifests: &[ManifestSpec],
    index: &RedfishIndex<'_>,
    contract: &prost_reflect::DescriptorPool,
    vocabulary: &crate::options::Vocabulary,
) -> Vec<Violation> {
    compile(manifests, index, contract, vocabulary).map_or_else(|violations| violations, |_| vec![])
}
