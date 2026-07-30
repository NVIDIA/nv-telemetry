// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Redfish acquisition.
//!
//! Owns HTTP transport, session handling, `OData` request construction, catalog
//! stages, and the leniency that real BMCs require. Field mapping is declared
//! in `manifests/` and compiled into `src/generated/` rather than written
//! here.
