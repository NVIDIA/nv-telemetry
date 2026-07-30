// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Content hashing.
//!
//! Emits a logical traversal of present known fields labeled by field number,
//! skipping fields annotated `collection_metadata`. The exclusion is
//! transitive: it applies through nested messages whether or not they are
//! hashable themselves, so a resource inside a hashable graph still has its
//! observation timestamp and entity tag skipped. Encoded bytes are never
//! hashed: protobuf encoding is not canonical across implementations, and
//! unknown fields would make equal graphs hash unequal. Absent fields
//! contribute nothing, so a schema revision that adds fields changes a hash
//! only when those fields carry data.
//!
//! Canonical sorting compares hash-visible fields first and uses collection
//! metadata only as a final tiebreaker. Elements that tie on hash-visible
//! fields contribute identical hash streams in either order, so the
//! tiebreaker keeps canonical bytes deterministic without letting collection
//! metadata move the hash.
