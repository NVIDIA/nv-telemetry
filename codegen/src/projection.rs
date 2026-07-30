// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Projection generation.
//!
//! Compiles manifests into extraction code. Paths are resolved against a
//! backend-neutral schema index, so a proto-native source resolving against a
//! descriptor pool and a Redfish source resolving against a generated index
//! share one code path.
