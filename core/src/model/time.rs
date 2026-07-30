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

use std::error::Error;
use std::fmt;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

/// Nanoseconds in one second.
///
/// Both types here split a signed quantity into whole seconds and a
/// non-negative sub-second remainder, and both hold that remainder below this
/// bound.
const NANOS_PER_SECOND: u32 = 1_000_000_000;

/// The unvalidated field pair both types are built from.
///
/// Deserialization goes through this so that a decoded value is admitted by
/// the same constructor a caller uses, rather than by a second copy of the
/// rule that could drift from it.
#[derive(Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize))]
struct SplitSeconds {
    seconds: i64,
    nanoseconds: u32,
}

impl TryFrom<SplitSeconds> for Timestamp {
    type Error = TimeError;

    fn try_from(value: SplitSeconds) -> Result<Self, Self::Error> {
        Self::new(value.seconds, value.nanoseconds)
    }
}

impl TryFrom<SplitSeconds> for DurationValue {
    type Error = TimeError;

    fn try_from(value: SplitSeconds) -> Result<Self, Self::Error> {
        Self::new(value.seconds, value.nanoseconds)
    }
}

/// Rejects a sub-second remainder that is not below one second, which is the
/// only thing either type can get wrong.
const fn check_nanoseconds(nanoseconds: u32) -> Result<(), TimeError> {
    if nanoseconds >= NANOS_PER_SECOND {
        return Err(TimeError::InvalidNanoseconds(nanoseconds));
    }
    Ok(())
}

/// A wall-clock timestamp represented as signed Unix seconds and nanoseconds.
///
/// This is an instant, not a span: see [`DurationValue`], which shares the
/// representation but is deliberately a separate type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "SplitSeconds"))]
pub struct Timestamp {
    seconds: i64,
    nanoseconds: u32,
}

impl Timestamp {
    pub const NANOS_PER_SECOND: u32 = NANOS_PER_SECOND;

    /// Builds a timestamp from a Unix second count and a sub-second offset.
    ///
    /// # Errors
    ///
    /// Returns [`TimeError::InvalidNanoseconds`] if the sub-second part is
    /// not below one second.
    pub const fn new(seconds: i64, nanoseconds: u32) -> Result<Self, TimeError> {
        if let Err(error) = check_nanoseconds(nanoseconds) {
            return Err(error);
        }
        Ok(Self {
            seconds,
            nanoseconds,
        })
    }

    pub const fn seconds(self) -> i64 {
        self.seconds
    }

    pub const fn nanoseconds(self) -> u32 {
        self.nanoseconds
    }

    /// Converts from a system clock reading.
    ///
    /// # Errors
    ///
    /// Returns [`TimeError::OutOfRange`] if the instant falls outside the
    /// range a 64-bit second count can address.
    pub fn from_system_time(value: SystemTime) -> Result<Self, TimeError> {
        match value.duration_since(UNIX_EPOCH) {
            Ok(duration) => Self::from_positive_duration(duration),
            Err(error) => Self::from_negative_duration(error.duration()),
        }
    }

    /// Converts to a system clock reading.
    ///
    /// # Errors
    ///
    /// Returns [`TimeError::OutOfRange`] if the instant is not
    /// representable as a [`SystemTime`] on this platform.
    pub fn to_system_time(self) -> Result<SystemTime, TimeError> {
        if self.seconds >= 0 {
            let seconds = u64::try_from(self.seconds).map_err(|_| TimeError::OutOfRange)?;
            return UNIX_EPOCH
                .checked_add(Duration::new(seconds, self.nanoseconds))
                .ok_or(TimeError::OutOfRange);
        }

        let absolute_seconds = self.seconds.unsigned_abs();
        let duration = if self.nanoseconds == 0 {
            Duration::new(absolute_seconds, 0)
        } else {
            let seconds = absolute_seconds
                .checked_sub(1)
                .ok_or(TimeError::OutOfRange)?;
            Duration::new(seconds, NANOS_PER_SECOND - self.nanoseconds)
        };
        UNIX_EPOCH
            .checked_sub(duration)
            .ok_or(TimeError::OutOfRange)
    }

    fn from_positive_duration(duration: Duration) -> Result<Self, TimeError> {
        let seconds = i64::try_from(duration.as_secs()).map_err(|_| TimeError::OutOfRange)?;
        Self::new(seconds, duration.subsec_nanos())
    }

    fn from_negative_duration(duration: Duration) -> Result<Self, TimeError> {
        let seconds = i64::try_from(duration.as_secs()).map_err(|_| TimeError::OutOfRange)?;
        if duration.subsec_nanos() == 0 {
            Self::new(-seconds, 0)
        } else {
            let seconds = seconds.checked_add(1).ok_or(TimeError::OutOfRange)?;
            Self::new(-seconds, NANOS_PER_SECOND - duration.subsec_nanos())
        }
    }
}

/// A signed span of time with nanosecond precision.
///
/// This shares [`Timestamp`]'s representation but not its meaning: one names
/// an instant and the other a length, and they must not substitute for each
/// other.
///
/// The nanosecond component is always a non-negative offset applied toward
/// positive infinity, as on `Timestamp`. Minus half a second is therefore
/// `seconds = -1, nanoseconds = 500_000_000`, and the derived ordering is
/// chronological because of that convention.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "SplitSeconds"))]
pub struct DurationValue {
    seconds: i64,
    nanoseconds: u32,
}

impl DurationValue {
    pub const NANOS_PER_SECOND: u32 = NANOS_PER_SECOND;

    /// Builds a duration from a second count and a sub-second offset.
    ///
    /// # Errors
    ///
    /// Returns [`TimeError::InvalidNanoseconds`] if the sub-second part is
    /// not below one second.
    pub const fn new(seconds: i64, nanoseconds: u32) -> Result<Self, TimeError> {
        if let Err(error) = check_nanoseconds(nanoseconds) {
            return Err(error);
        }
        Ok(Self {
            seconds,
            nanoseconds,
        })
    }

    pub const fn seconds(self) -> i64 {
        self.seconds
    }

    pub const fn nanoseconds(self) -> u32 {
        self.nanoseconds
    }

    /// Returns the total signed duration in nanoseconds.
    pub const fn as_nanos(self) -> i128 {
        self.seconds as i128 * NANOS_PER_SECOND as i128 + self.nanoseconds as i128
    }
}

/// The wall-clock interval during which an observation was acquired.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "Span"))]
pub struct ObservationWindow {
    started_at: Timestamp,
    completed_at: Timestamp,
}

impl ObservationWindow {
    /// Builds a window spanning two instants.
    ///
    /// # Errors
    ///
    /// Returns [`TimeError::WindowEndsBeforeStart`] if the end precedes
    /// the start.
    pub const fn new(started_at: Timestamp, completed_at: Timestamp) -> Result<Self, TimeError> {
        if timestamp_after(started_at, completed_at) {
            return Err(TimeError::WindowEndsBeforeStart);
        }
        Ok(Self {
            started_at,
            completed_at,
        })
    }

    pub const fn point(observed_at: Timestamp) -> Self {
        Self {
            started_at: observed_at,
            completed_at: observed_at,
        }
    }

    pub const fn started_at(self) -> Timestamp {
        self.started_at
    }

    pub const fn completed_at(self) -> Timestamp {
        self.completed_at
    }
}

const fn timestamp_after(left: Timestamp, right: Timestamp) -> bool {
    left.seconds > right.seconds
        || (left.seconds == right.seconds && left.nanoseconds > right.nanoseconds)
}

/// A rejected timestamp, duration, or observation window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TimeError {
    InvalidNanoseconds(u32),
    WindowEndsBeforeStart,
    OutOfRange,
}

impl fmt::Display for TimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNanoseconds(value) => {
                write!(
                    formatter,
                    "nanoseconds must be below 1,000,000,000, got {value}"
                )
            }
            Self::WindowEndsBeforeStart => {
                formatter.write_str("observation window ends before it starts")
            }
            Self::OutOfRange => formatter.write_str("timestamp is outside the supported range"),
        }
    }
}

impl Error for TimeError {}

/// The unvalidated field pair an [`ObservationWindow`] is built from.
#[derive(Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize))]
struct Span {
    started_at: Timestamp,
    completed_at: Timestamp,
}

impl TryFrom<Span> for ObservationWindow {
    type Error = TimeError;

    fn try_from(value: Span) -> Result<Self, Self::Error> {
        Self::new(value.started_at, value.completed_at)
    }
}
