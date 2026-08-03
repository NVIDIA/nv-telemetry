// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! What a validator says when it refuses.
//!
//! One error type for every validated construction, whether the message was
//! built or decoded — the same checks run on both paths, so the same error
//! comes back from both. It carries the path to the offending field, built
//! outward as the error propagates up the tree, because "a subject id was
//! empty" is not actionable in a batch of sixty-five thousand items and
//! `resources[41].subject.id` is.
//!
//! First failure only. A validator that collected everything wrong with a
//! batch would do unbounded work deciding to refuse it; an operator who needs
//! the full list can fix and re-run, and the path makes each round direct.

use std::fmt;

/// A message that failed validation, with the path that failed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invalid {
    /// Dotted path from the message being constructed to the field at fault,
    /// with indexes into repeated fields: `resources[41].subject.id`.
    path: String,
    violation: Violation,
}

/// What was wrong at the path.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Violation {
    /// A `required` field, or a required oneof, carried nothing.
    Absent,
    /// A `non_empty` string or bytes value was empty.
    Empty,
    /// A `max_len` bound was exceeded.
    TooLong {
        /// The schema's bound, in bytes.
        limit: u32,
        /// The offending length.
        actual: usize,
    },
    /// A `max_items` bound was exceeded.
    TooMany {
        /// The schema's bound.
        limit: u32,
        /// The offending count.
        actual: usize,
    },
    /// A `finite` field carried `NaN` or an infinity.
    NotFinite,
    /// A `reject_unspecified` enum field carried the zero value.
    Unspecified,
    /// Two elements collided on a uniqueness key, or a map carried one key
    /// twice.
    Duplicate,
    /// A `max_depth` bound was exceeded.
    TooDeep {
        /// The schema's bound, in logical levels.
        limit: u32,
    },
    /// A cross-field rule — one the annotation vocabulary deliberately cannot
    /// express — was broken. The text is the rule, stated as the schema
    /// comments state it.
    Rule(&'static str),
}

impl Invalid {
    /// A violation at `field` of the message under construction.
    #[must_use]
    pub(crate) fn field(field: &str, violation: Violation) -> Self {
        Self {
            path: field.to_owned(),
            violation,
        }
    }

    /// A violation at one element of the repeated field `field`.
    #[must_use]
    pub(crate) fn element(field: &str, index: usize, violation: Violation) -> Self {
        Self {
            path: format!("{field}[{index}]"),
            violation,
        }
    }

    /// Prefixes the path with the field this error bubbled out of.
    #[must_use]
    pub(crate) fn at(mut self, segment: &str) -> Self {
        if self.path.is_empty() {
            segment.clone_into(&mut self.path);
        } else {
            self.path = format!("{segment}.{}", self.path);
        }
        self
    }

    /// Prefixes the path with an element of a repeated field.
    #[must_use]
    pub(crate) fn at_index(self, segment: &str, index: usize) -> Self {
        self.at(&format!("{segment}[{index}]"))
    }

    /// Dotted path from the constructed message to the field at fault.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// What was wrong there.
    #[must_use]
    pub fn violation(&self) -> &Violation {
        &self.violation
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absent => f.write_str("required but absent"),
            Self::Empty => f.write_str("present but empty"),
            Self::TooLong { limit, actual } => {
                write!(f, "{actual} bytes long, over the schema's bound of {limit}")
            }
            Self::TooMany { limit, actual } => {
                write!(f, "{actual} elements, over the schema's bound of {limit}")
            }
            Self::NotFinite => f.write_str("not a finite number"),
            Self::Unspecified => f.write_str("carries the unspecified value"),
            Self::Duplicate => f.write_str("duplicates another element's identity"),
            Self::TooDeep { limit } => {
                write!(f, "nested beyond the schema's bound of {limit} levels")
            }
            Self::Rule(rule) => f.write_str(rule),
        }
    }
}

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}`: {}", self.path, self.violation)
    }
}

impl std::error::Error for Invalid {}

/// Why bytes did not become a validated message.
///
/// The two layers are deliberately distinct facts: `Malformed` bytes are not
/// protobuf at all, while `Invalid` bytes decoded fine and then broke the
/// contract — the difference between line noise and a producer that disagrees
/// about the rules.
#[derive(Debug)]
#[non_exhaustive]
pub enum DecodeError {
    /// The bytes are not a valid protobuf message.
    Malformed(prost::DecodeError),
    /// The message decoded but failed validation.
    Invalid(Invalid),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(error) => write!(f, "not a valid protobuf message: {error}"),
            Self::Invalid(error) => write!(f, "decoded but invalid: {error}"),
        }
    }
}

impl std::error::Error for DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Malformed(error) => Some(error),
            Self::Invalid(error) => Some(error),
        }
    }
}

/// The violation for a value over a byte bound, if it is.
pub(crate) fn too_long(actual: usize, limit: u32) -> Option<Violation> {
    exceeds(actual, limit).then_some(Violation::TooLong { limit, actual })
}

/// The violation for a collection over an element bound, if it is.
pub(crate) fn too_many(actual: usize, limit: u32) -> Option<Violation> {
    exceeds(actual, limit).then_some(Violation::TooMany { limit, actual })
}

fn exceeds(actual: usize, limit: u32) -> bool {
    match u64::try_from(actual) {
        Ok(actual) => actual > u64::from(limit),
        // Unreachable on any supported target; failing closed keeps an exotic
        // one from waving an oversized value through.
        Err(_) => true,
    }
}
