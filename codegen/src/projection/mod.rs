// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Projection generation.
//!
//! Manifests under `sources/*/manifests/` are loaded ([`spec`]), resolved
//! against the CSDL index ([`index`]) and the contract pool, and checked —
//! every declaration the compiler cannot honor is an error ([`lint`]).
//! [`compile`] wraps a clean check into the receipt [`emit`](mod@emit)
//! requires, so emission cannot run on unchecked manifests; what emission
//! itself refuses are compiler capability limits, loud at `make codegen`.

pub mod emit;
pub mod index;
pub mod lint;
pub mod spec;

mod location;

pub use emit::emit;
pub use index::Bundle;
pub use index::IndexError;
pub use index::RedfishIndex;
pub use index::ResolvedField;
pub use index::Shape;
pub use index::Step;
pub use lint::check;
pub use lint::Violation;
use prost_reflect::DescriptorPool;
pub use spec::load;
pub use spec::ManifestError;
pub use spec::ManifestSpec;

use crate::options::Vocabulary;

/// Manifests that passed every check: the only value [`fn@emit`] accepts.
/// The private field keeps a caller from constructing one around unchecked
/// input.
#[derive(Clone, Copy, Debug)]
pub struct Checked<'a> {
    manifests: &'a [ManifestSpec],
}

impl Checked<'_> {
    pub(crate) fn manifests(&self) -> &[ManifestSpec] {
        self.manifests
    }
}

/// Checks manifests and returns the receipt emission requires.
///
/// # Errors
///
/// Every violation, in deterministic declaration order.
pub fn compile<'a>(
    manifests: &'a [ManifestSpec],
    index: &RedfishIndex<'_>,
    contract: &DescriptorPool,
    vocabulary: &Vocabulary,
) -> Result<Checked<'a>, Vec<Violation>> {
    let violations = check(manifests, index, contract, vocabulary);
    if violations.is_empty() {
        Ok(Checked { manifests })
    } else {
        Err(violations)
    }
}
