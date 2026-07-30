// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Validated wrapper generation.
//!
//! For each message annotated `validated`, emits a newtype holding the wire
//! message privately, generated builders, fallible conversion from the wire
//! type, and read-through accessors. Because the inner representation is
//! private, it can change without breaking the public contract.
