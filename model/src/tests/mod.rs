// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The model's test suite, one module per concern.
//!
//! Unit tests rather than integration tests, because the generated module is
//! crate-internal: making it public so a `tests/` file could reach it would
//! trade the guarantee for the convenience of testing it.

mod boundary;
mod canonical;
mod encoding;
mod properties;
mod values;
mod wire_properties;
