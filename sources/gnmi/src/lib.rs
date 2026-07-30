// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! `gNMI` acquisition.
//!
//! `gNMI` is protobuf-native, so decode comes from prost and there is no
//! vendor-leniency layer. Projection still resolves against a generated index:
//! the descriptor pool describes the transport envelope, not the data model,
//! whose paths and types come from YANG.
//!
//! Subscriptions are long-lived streams, which makes this the source that
//! settles how streamed acquisition is admitted by the dispatcher.
