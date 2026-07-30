#!/usr/bin/env bash
# SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
# SPDX-License-Identifier: Apache-2.0
#
# Rejects schema changes that strand a consumer decoding data written against
# the baseline.
#
# Lives in a script rather than in a Makefile recipe for two reasons. GNU Make
# on Windows chooses its own recipe shell — it looks for sh on PATH and falls
# back to cmd.exe — so a recipe containing shell syntax works or breaks
# depending on the machine. And the logic below has to distinguish three
# states, which is more than a recipe should carry.
set -euo pipefail

buf=${BUF:-buf}
baseline=${1:?usage: proto-breaking.sh <baseline-ref>}

# The current tree must build, first and unconditionally. An empty or
# unbuildable contract is a failure and never a reason to skip: reporting
# "nothing to compare" when the contract was deleted would be exactly the
# silent pass this gate exists to prevent.
if ! output=$("$buf" build 2>&1); then
  printf '%s\n' "$output" >&2
  echo "proto-breaking: the current schema does not build, so compatibility was not checked" >&2
  exit 1
fi

if ! git rev-parse --verify --quiet "$baseline" >/dev/null 2>&1; then
  # In CI a missing baseline means the checkout is wrong — typically a shallow
  # clone without origin/main — and silently skipping would disable the gate
  # for every pull request.
  if [ -n "${CI:-}" ]; then
    echo "proto-breaking: baseline '$baseline' does not exist in this checkout." >&2
    echo "  CI needs fetch-depth: 0 for the comparison to be possible." >&2
    exit 1
  fi
  echo "proto-breaking: baseline '$baseline' does not exist; nothing to compare against yet"
  exit 0
fi

# Whether there is anything to compare against is decided by building the
# baseline ALONE. Deciding it from the combined output of `buf breaking` cannot
# tell an empty baseline from an empty current tree, because buf reports both
# with the same message.
if ! output=$("$buf" build ".git#ref=$baseline" 2>&1); then
  if printf '%s' "$output" | grep -q 'had no .proto files'; then
    echo "proto-breaking: no schema at '$baseline'; nothing to compare against yet"
    exit 0
  fi
  printf '%s\n' "$output" >&2
  echo "proto-breaking: baseline '$baseline' does not build" >&2
  exit 1
fi

# exec so buf's own exit code and diagnostics reach the caller unaltered.
exec "$buf" breaking --against ".git#ref=$baseline"
