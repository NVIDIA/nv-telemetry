// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Validator generation.
//!
//! Emits the checks that a validated wrapper runs on construction and on
//! decode alike, so a decoded batch passes exactly what a built one does and
//! the model cannot produce what it cannot consume.
