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
#[derive(Clone, Copy, Debug)]
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
            return Ok(Self::normalized(value));
        }
        if value.is_nan() {
            Err(NonFiniteError::NaN)
        } else if value.is_sign_positive() {
            Err(NonFiniteError::PositiveInfinity)
        } else {
            Err(NonFiniteError::NegativeInfinity)
        }
    }

    /// The one way a computed finite value becomes a `Finite`.
    ///
    /// The sole exception is [`ZERO`](Self::ZERO), the literal this returns
    /// for zero and therefore already in normal form; routing it back through
    /// here would recurse in const evaluation.
    ///
    /// Collapsing negative zero here is what makes it the same value as
    /// positive zero. Everything below reads the bits, so an uncollapsed
    /// `-0.0` would compare, order, and hash as a distinct value, and a
    /// device reporting one and then the other would look changed.
    const fn normalized(value: f64) -> Self {
        if value == 0.0 {
            return Self::ZERO;
        }
        Self(value)
    }

    pub const fn get(self) -> f64 {
        self.0
    }

    /// The single key equality, ordering, and hashing all read.
    ///
    /// This is the IEEE 754 `totalOrder` key that [`f64::total_cmp`]
    /// compares: a negative value keeps its negative key so that it sorts
    /// below every non-negative one, and its remaining bits are inverted so
    /// that a larger magnitude sorts lower.
    ///
    /// Reading one key is what makes the three agree. Hashing the bits
    /// separately would agree only for as long as the two kept calling the
    /// same values equal, which nothing states or checks.
    const fn total_order_key(self) -> i64 {
        let bits = i64::from_ne_bytes(self.0.to_ne_bytes());
        if bits < 0 {
            bits ^ i64::MAX
        } else {
            bits
        }
    }
}

/// Defined as the ordering rather than derived, so that equality cannot
/// disagree with it.
impl PartialEq for Finite {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
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
        self.total_order_key().cmp(&other.total_order_key())
    }
}

impl Hash for Finite {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.total_order_key().hash(state);
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
        Self::normalized(f64::from(value))
    }
}

impl From<u32> for Finite {
    fn from(value: u32) -> Self {
        Self::normalized(f64::from(value))
    }
}

/// The three ways an `f64` can fail to be finite, which is all of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NonFiniteError {
    NaN,
    PositiveInfinity,
    NegativeInfinity,
}

impl fmt::Display for NonFiniteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let classification = match self {
            Self::NaN => "NaN",
            Self::PositiveInfinity => "positive infinity",
            Self::NegativeInfinity => "negative infinity",
        };
        write!(
            formatter,
            "observation values must be finite, got {classification}"
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
    fn equality_ordering_and_hashing_agree() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::Hash as _;
        use std::hash::Hasher as _;

        let digest = |value: Finite| {
            let mut hasher = DefaultHasher::new();
            value.hash(&mut hasher);
            hasher.finish()
        };

        for pair in [
            (Finite::new(-0.0).unwrap(), Finite::from(0_i32)),
            (Finite::from(0_u32), Finite::ZERO),
            (Finite::from(-1_i32), Finite::new(-1.0).unwrap()),
        ] {
            assert_eq!(pair.0, pair.1);
            assert!(pair.0.cmp(&pair.1).is_eq());
            assert_eq!(digest(pair.0), digest(pair.1));
        }
    }

    /// The sort key must order values exactly as the standard library does.
    ///
    /// It is written out here rather than delegated to `total_cmp` so that
    /// hashing can read the same key; this pins the two together.
    #[test]
    fn the_sort_key_orders_as_total_cmp_does() {
        let values = [
            f64::MIN,
            -1e300,
            -1.5,
            -1.0,
            -f64::MIN_POSITIVE,
            0.0,
            f64::MIN_POSITIVE,
            1.0,
            1.5,
            1e300,
            f64::MAX,
        ];

        for left in values {
            for right in values {
                let left = Finite::new(left).unwrap();
                let right = Finite::new(right).unwrap();
                assert_eq!(
                    left.cmp(&right),
                    left.get().total_cmp(&right.get()),
                    "{left} vs {right}"
                );
            }
        }
    }

    #[test]
    fn non_finite_is_rejected() {
        assert_eq!(Finite::new(f64::NAN), Err(NonFiniteError::NaN));
        assert_eq!(
            Finite::new(f64::INFINITY),
            Err(NonFiniteError::PositiveInfinity)
        );
        assert_eq!(
            Finite::new(f64::NEG_INFINITY),
            Err(NonFiniteError::NegativeInfinity)
        );
    }
}
