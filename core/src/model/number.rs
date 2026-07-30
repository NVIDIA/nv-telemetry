// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::cmp::Ordering;
use std::error::Error;
use std::fmt;
use std::hash::Hash;
use std::hash::Hasher;

/// A floating point observation value that is never `NaN` or infinite.
///
/// Consumers diff successive observations to detect change, which needs exact
/// comparison. `NaN` is unequal to itself, so one non-finite value would make
/// a resource appear changed on every comparison and would bar the model from
/// implementing `Eq` and `Hash`.
///
/// An unmeasurable quantity is expressed as absence, never a sentinel float.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(into = "f64", try_from = "f64"))]
pub struct Finite(f64);

impl Finite {
    pub const ZERO: Self = Self(0.0);

    /// Accepts a finite value, normalising negative zero.
    ///
    /// # Errors
    ///
    /// Returns [`NonFiniteError`] if the value is `NaN` or an infinity.
    pub const fn new(value: f64) -> Result<Self, NonFiniteError> {
        if value.is_finite() {
            // Collapse negative zero so that equality and hashing agree.
            if value == 0.0 {
                return Ok(Self::ZERO);
            }
            return Ok(Self(value));
        }
        Err(NonFiniteError { value })
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

impl Eq for Finite {}

impl PartialOrd for Finite {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Finite {
    fn cmp(&self, other: &Self) -> Ordering {
        // Total: both operands are finite and negative zero is normalized.
        self.0.partial_cmp(&other.0).unwrap_or(Ordering::Equal)
    }
}

impl Hash for Finite {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

impl fmt::Display for Finite {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, formatter)
    }
}

impl TryFrom<f64> for Finite {
    type Error = NonFiniteError;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Finite> for f64 {
    fn from(value: Finite) -> Self {
        value.0
    }
}

impl From<i32> for Finite {
    fn from(value: i32) -> Self {
        Self(f64::from(value))
    }
}

impl From<u32> for Finite {
    fn from(value: u32) -> Self {
        Self(f64::from(value))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct NonFiniteError {
    /// The rejected value, which may be `NaN` and so compare unequal to itself.
    pub value: f64,
}

impl fmt::Display for NonFiniteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "observation values must be finite, got {}",
            self.value
        )
    }
}

impl Error for NonFiniteError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn const_construction_is_available() {
        const VALUE: Finite = match Finite::new(1.5) {
            Ok(value) => value,
            Err(_) => panic!("finite"),
        };
        assert_eq!(VALUE.get(), 1.5);
    }

    #[test]
    fn negative_zero_is_normalized() {
        let negative = Finite::new(-0.0).unwrap();
        assert_eq!(negative, Finite::ZERO);
        assert!(negative.get().is_sign_positive());
    }

    #[test]
    fn non_finite_is_rejected() {
        assert!(Finite::new(f64::NAN).is_err());
        assert!(Finite::new(f64::INFINITY).is_err());
        assert!(Finite::new(f64::NEG_INFINITY).is_err());
    }
}
