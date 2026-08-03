// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The finite floating-point type behind every `finite` annotation.
//!
//! A reading is compared and hashed exactly, so the model needs a float with
//! total equality — which `f64` refuses to be, on two counts. `NaN` is not
//! equal to itself, and `-0.0 == 0.0` while their bits differ, so a hash over
//! bits would call equal values unequal. Excluding non-finite values settles
//! the first (they are fabrications in a telemetry reading anyway, which is
//! why the schema rejects them), and normalizing the zero at construction
//! settles the second: one value, one representation, the same rule the
//! schema applies to timestamps.

use std::fmt;

/// A finite `f64`: no `NaN`, no infinities, and exactly one zero.
///
/// The only way in is [`Finite::new`], so every value of this type upholds
/// the invariant and the manual `Eq`, `Ord`, and `Hash` implementations below
/// are total.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Finite(f64);

impl Finite {
    /// Admits `value` if it is finite, normalizing `-0.0` to `0.0`.
    ///
    /// Returns `None` for `NaN` and the infinities; the caller decides what
    /// field that makes invalid.
    #[must_use]
    pub fn new(value: f64) -> Option<Self> {
        if !value.is_finite() {
            return None;
        }
        // `+ 0.0` maps -0.0 to 0.0 and changes nothing else.
        Some(Self(value + 0.0))
    }

    /// The value.
    #[must_use]
    pub fn get(self) -> f64 {
        self.0
    }
}

// Sound because the constructor excludes `NaN`: `PartialEq` on the remaining
// values is reflexive.
impl Eq for Finite {}

impl PartialOrd for Finite {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Finite {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Total for the same reason `Eq` is: no `NaN` can reach this.
        self.0
            .partial_cmp(&other.0)
            .expect("Finite holds no NaN, so comparison is total")
    }
}

impl std::hash::Hash for Finite {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // Consistent with `Eq`: equal finite values have equal bits once the
        // constructor has collapsed the two zeros.
        state.write_u64(self.0.to_bits());
    }
}

impl fmt::Display for Finite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<Finite> for f64 {
    fn from(value: Finite) -> Self {
        value.get()
    }
}
