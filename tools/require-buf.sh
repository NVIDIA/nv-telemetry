#!/usr/bin/env bash
# SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
# SPDX-License-Identifier: Apache-2.0
#
# Checks that buf is present and is the version the pipeline pins.
#
# The version matters beyond reproducibility: `buf lint`'s STANDARD category
# and `buf format`'s output both change between releases, so a developer
# running `make fmt` with a different buf reformats the schema in a way CI then
# rejects, for reasons neither side explains.
set -euo pipefail

buf=${BUF:-buf}
want=${1:-}

if ! command -v "$buf" >/dev/null 2>&1; then
  echo "buf not found. Install it: https://buf.build/docs/cli/installation" >&2
  exit 1
fi

if [ -z "$want" ]; then
  exit 0
fi

have=$("$buf" --version 2>/dev/null || echo unknown)
if [ "$have" != "$want" ]; then
  echo "buf $have found, but this pipeline pins $want." >&2
  echo "  Formatting and lint rules differ between releases, so a mismatch" >&2
  echo "  shows up as CI rejecting a tree that passed locally." >&2
  echo "  Install $want, or set buf-version= to disable this check." >&2
  exit 1
fi
