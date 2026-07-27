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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A wall-clock timestamp represented as signed Unix seconds and nanoseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Timestamp {
    seconds: i64,
    nanoseconds: u32,
}

impl Timestamp {
    pub const NANOS_PER_SECOND: u32 = 1_000_000_000;

    /// Builds a timestamp from a Unix second count and a sub-second offset.
    ///
    /// # Errors
    ///
    /// Returns [`TimestampError::InvalidNanoseconds`] if the sub-second part is
    /// not below one second, which would make two timestamps compare wrongly.
    pub const fn new(seconds: i64, nanoseconds: u32) -> Result<Self, TimestampError> {
        if nanoseconds >= Self::NANOS_PER_SECOND {
            return Err(TimestampError::InvalidNanoseconds(nanoseconds));
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
    /// Returns [`TimestampError::OutOfRange`] if the instant falls outside the
    /// range a 64-bit second count can address.
    pub fn from_system_time(value: SystemTime) -> Result<Self, TimestampError> {
        match value.duration_since(UNIX_EPOCH) {
            Ok(duration) => Self::from_positive_duration(duration),
            Err(error) => Self::from_negative_duration(error.duration()),
        }
    }

    /// Converts to a system clock reading.
    ///
    /// # Errors
    ///
    /// Returns [`TimestampError::OutOfRange`] if the instant is not
    /// representable as a [`SystemTime`] on this platform.
    pub fn to_system_time(self) -> Result<SystemTime, TimestampError> {
        if self.seconds >= 0 {
            let seconds = u64::try_from(self.seconds).map_err(|_| TimestampError::OutOfRange)?;
            return UNIX_EPOCH
                .checked_add(Duration::new(seconds, self.nanoseconds))
                .ok_or(TimestampError::OutOfRange);
        }

        let absolute_seconds = self.seconds.unsigned_abs();
        let duration = if self.nanoseconds == 0 {
            Duration::new(absolute_seconds, 0)
        } else {
            let seconds = absolute_seconds
                .checked_sub(1)
                .ok_or(TimestampError::OutOfRange)?;
            Duration::new(seconds, Self::NANOS_PER_SECOND - self.nanoseconds)
        };
        UNIX_EPOCH
            .checked_sub(duration)
            .ok_or(TimestampError::OutOfRange)
    }

    fn from_positive_duration(duration: Duration) -> Result<Self, TimestampError> {
        let seconds = i64::try_from(duration.as_secs()).map_err(|_| TimestampError::OutOfRange)?;
        Self::new(seconds, duration.subsec_nanos())
    }

    fn from_negative_duration(duration: Duration) -> Result<Self, TimestampError> {
        let seconds = i64::try_from(duration.as_secs()).map_err(|_| TimestampError::OutOfRange)?;
        if duration.subsec_nanos() == 0 {
            Self::new(-seconds, 0)
        } else {
            let seconds = seconds.checked_add(1).ok_or(TimestampError::OutOfRange)?;
            Self::new(-seconds, Self::NANOS_PER_SECOND - duration.subsec_nanos())
        }
    }
}

/// The wall-clock interval during which an observation was acquired.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ObservationWindow {
    started_at: Timestamp,
    completed_at: Timestamp,
}

impl ObservationWindow {
    /// Builds a window spanning two instants.
    ///
    /// # Errors
    ///
    /// Returns [`TimestampError::WindowEndsBeforeStart`] if the end precedes the
    /// start, which would describe a collection that finished before it began.
    pub const fn new(
        started_at: Timestamp,
        completed_at: Timestamp,
    ) -> Result<Self, TimestampError> {
        if timestamp_after(started_at, completed_at) {
            return Err(TimestampError::WindowEndsBeforeStart);
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimestampError {
    InvalidNanoseconds(u32),
    WindowEndsBeforeStart,
    OutOfRange,
}

impl fmt::Display for TimestampError {
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

impl Error for TimestampError {}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Timestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Representation {
            seconds: i64,
            nanoseconds: u32,
        }

        let value = <Representation as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(value.seconds, value.nanoseconds).map_err(serde::de::Error::custom)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for ObservationWindow {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Representation {
            started_at: Timestamp,
            completed_at: Timestamp,
        }

        let value = <Representation as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(value.started_at, value.completed_at).map_err(serde::de::Error::custom)
    }
}
