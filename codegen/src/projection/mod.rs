// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Projection generation.
//!
//! Compiles manifests into extraction code, resolving paths against a
//! backend-neutral schema index. What exists today is checking: manifests
//! under `sources/*/manifests/` are loaded ([`spec`]), resolved against the
//! CSDL index ([`index`]) and the contract pool, and every declaration the
//! compiler cannot honor is an error ([`lint`]). Emission follows; the
//! checked manifest is its input.

pub mod index;
pub mod lint;
pub mod spec;

pub use index::Bundle;
pub use index::IndexError;
pub use index::RedfishIndex;
pub use index::ResolvedField;
pub use lint::check;
pub use lint::Violation;
pub use spec::load;
pub use spec::ManifestError;
pub use spec::ManifestSpec;
