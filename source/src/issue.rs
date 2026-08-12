// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Projection issues: what one source field failed to become.
//!
//! Missing and invalid are different facts — silence versus an answer that
//! cannot be used — and both are structured issues attached to the
//! acquisition result rather than log lines. An issue describes what is
//! *not* in any batch, which is why it rides beside the batches instead of
//! inside one: attaching issues to a batch would force fabricating an empty
//! batch just to carry them, and issues never become fabricated
//! observations.
//!
//! Paths locate the source field in the source's own schema grammar —
//! `Sensor.Reading`, `Sensors[3].Status.Health` — built outward as the issue
//! propagates, exactly as [`Invalid`](nv_telemetry_model::Invalid) builds
//! model paths. The two read alike on purpose: locating a projection issue
//! and locating a validation violation are the same skill.

use std::fmt;

/// Why one source field did not become an observation.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProjectionIssueKind {
    /// The device did not report a field the projection requires.
    MissingRequired,
    /// The device reported the field unusably; the detail quotes the
    /// failure. An unresolvable subject scope is this kind, not a kind of
    /// its own: the location was answered and the answer cannot be used.
    Invalid {
        /// What made the reported value unusable.
        detail: String,
    },
}

/// A source field that failed to project, located by source-schema path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectionIssue {
    /// Dotted path in the source's schema grammar, with indexes into
    /// repeated source fields.
    path: String,
    kind: ProjectionIssueKind,
}

impl ProjectionIssue {
    /// An issue for a field the device did not report.
    #[must_use]
    pub fn missing(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            kind: ProjectionIssueKind::MissingRequired,
        }
    }

    /// An issue for a field the device reported unusably.
    #[must_use]
    pub fn invalid(path: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            kind: ProjectionIssueKind::Invalid {
                detail: detail.into(),
            },
        }
    }

    /// Prefixes the path with the segment this issue bubbled out of.
    #[must_use]
    pub fn at(mut self, segment: &str) -> Self {
        if self.path.is_empty() {
            segment.clone_into(&mut self.path);
        } else {
            self.path = format!("{segment}.{}", self.path);
        }
        self
    }

    /// Prefixes the path with an element of a repeated source field.
    #[must_use]
    pub fn at_index(self, segment: &str, index: usize) -> Self {
        self.at(&format!("{segment}[{index}]"))
    }

    /// Dotted source-schema path to the field at fault.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// What kept the field from projecting.
    #[must_use]
    pub fn kind(&self) -> &ProjectionIssueKind {
        &self.kind
    }
}

impl fmt::Display for ProjectionIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ProjectionIssueKind::MissingRequired => {
                write!(f, "`{}`: required but not reported", self.path)
            }
            ProjectionIssueKind::Invalid { detail } => {
                write!(f, "`{}`: reported but unusable: {detail}", self.path)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_build_outward_like_model_paths() {
        let issue = ProjectionIssue::invalid("Reading", "not a finite number")
            .at_index("Sensors", 3)
            .at("Chassis");
        assert_eq!(issue.path(), "Chassis.Sensors[3].Reading");
        assert_eq!(
            issue.to_string(),
            "`Chassis.Sensors[3].Reading`: reported but unusable: not a finite number"
        );
    }

    #[test]
    fn a_missing_field_states_the_silence() {
        let issue = ProjectionIssue::missing("Id");
        assert_eq!(issue.kind(), &ProjectionIssueKind::MissingRequired);
        assert_eq!(issue.to_string(), "`Id`: required but not reported");
    }
}
