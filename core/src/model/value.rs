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

//! Shared machinery for the model's typed value enums.

/// Implements the conversions shared by the model's value enums.
///
/// A caller filling any of them reaches for the same conversions, and all of
/// them have to refuse a non-finite float through the same checked
/// constructor.
///
/// `numeric` covers the `I64`, `U64`, and `F64` variants that
/// [`NumericValue`](super::NumericValue),
/// [`AttrValue`](super::AttrValue), and
/// [`PropertyValue`](super::PropertyValue) all carry. `scalar` is `numeric`
/// plus the `String` and `Bool` variants that only the latter two have, so
/// nothing needs both arms.
///
/// Model types are named by full path, so a call site imports only what its
/// own text uses.
macro_rules! value_conversions {
    (numeric $value:ident) => {
        impl $value {
            /// Builds a floating point value, rejecting non-finite input.
            ///
            /// # Errors
            ///
            /// Returns [`NonFiniteError`](crate::model::NonFiniteError) if
            /// the value is `NaN` or an infinity.
            pub const fn f64(value: f64) -> Result<Self, crate::model::NonFiniteError> {
                match crate::model::Finite::new(value) {
                    Ok(value) => Ok(Self::F64(value)),
                    Err(error) => Err(error),
                }
            }
        }

        impl From<i64> for $value {
            fn from(value: i64) -> Self {
                Self::I64(value)
            }
        }

        impl From<u64> for $value {
            fn from(value: u64) -> Self {
                Self::U64(value)
            }
        }

        impl From<crate::model::Finite> for $value {
            fn from(value: crate::model::Finite) -> Self {
                Self::F64(value)
            }
        }

        impl TryFrom<f64> for $value {
            type Error = crate::model::NonFiniteError;

            fn try_from(value: f64) -> Result<Self, Self::Error> {
                Self::f64(value)
            }
        }
    };
    // A superset of `numeric`, which it invokes: a value enum takes one arm
    // or the other, never both.
    (scalar $value:ident) => {
        value_conversions!(numeric $value);

        impl From<crate::model::Name> for $value {
            fn from(value: crate::model::Name) -> Self {
                Self::String(value)
            }
        }

        impl From<String> for $value {
            fn from(value: String) -> Self {
                Self::String(value.into())
            }
        }

        impl From<&str> for $value {
            fn from(value: &str) -> Self {
                Self::String(value.into())
            }
        }

        impl From<bool> for $value {
            fn from(value: bool) -> Self {
                Self::Bool(value)
            }
        }
    };
}

pub(super) use value_conversions;
