// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The nv-telemetry protobuf contract.
//!
//! This crate holds the `.proto` files and exposes them compiled, so that one
//! artifact serves the schema compiler, reflection consumers, and builds in
//! other languages.
//!
//! Three packages are versioned independently of this crate:
//!
//! | Package                   | Contents                                      |
//! | ------------------------- | --------------------------------------------- |
//! | `nv.telemetry.v1`         | observation contract                          |
//! | `nv.telemetry.options.v1` | annotation vocabulary (extension range 52000) |
//! | `nv.telemetry.mapping.v1` | manifest schema for projections               |

/// The compiled `FileDescriptorSet`, including custom options.
///
/// Encoded rather than parsed so that this crate stays free of a protobuf
/// runtime dependency; callers that need reflection decode it into a
/// descriptor pool themselves.
///
/// The set is self-contained: it carries its imports, including
/// `google/protobuf/descriptor.proto`, because without them a consumer could
/// name the annotation extensions but not resolve them. The cost is that
/// merging it into a pool that already holds a different build of
/// `descriptor.proto` is a same-name conflict. Decode it into its own pool
/// unless the surrounding pool is known to match.
pub const DESCRIPTOR_SET: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/nv_telemetry.bin"));
