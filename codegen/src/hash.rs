// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Content hashing.
//!
//! Emits a logical traversal of present known fields labeled by field number.
//! Encoded bytes are never hashed: protobuf encoding is not canonical across
//! implementations, and unknown fields would make equal graphs hash unequal.
//! Absent fields contribute nothing, so a schema revision that adds fields
//! changes a hash only when those fields carry data.
