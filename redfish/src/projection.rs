// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

use std::fmt;

/// Converts one typed source value into one telemetry value.
pub trait Project<Input, Context = ()> {
    type Output;

    fn project(input: &Input, context: &Context) -> ProjectionResult<Self::Output>;
}

/// Why a projection field could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectionIssueKind {
    MissingRequired,
    Invalid { detail: String },
}

/// A source field that prevented a projection from producing output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectionIssue {
    path: &'static str,
    kind: ProjectionIssueKind,
}

impl ProjectionIssue {
    pub const fn new(path: &'static str, kind: ProjectionIssueKind) -> Self {
        Self { path, kind }
    }

    pub const fn path(&self) -> &'static str {
        self.path
    }

    pub const fn kind(&self) -> &ProjectionIssueKind {
        &self.kind
    }
}

/// Result of projecting one source value.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectionResult<T> {
    output: Option<T>,
    issues: Box<[ProjectionIssue]>,
}

impl<T> ProjectionResult<T> {
    pub fn new(output: Option<T>, issues: Vec<ProjectionIssue>) -> Self {
        Self {
            output,
            issues: issues.into_boxed_slice(),
        }
    }

    pub fn output(&self) -> Option<&T> {
        self.output.as_ref()
    }

    pub fn issues(&self) -> &[ProjectionIssue] {
        &self.issues
    }

    pub fn into_parts(self) -> (Option<T>, Box<[ProjectionIssue]>) {
        (self.output, self.issues)
    }
}

/// What one source field yielded, before a projection judges whether it can
/// proceed without it.
///
/// A field is *missing* when the device said nothing and *invalid* when it
/// answered unusably. A consumer needs the difference: a `NaN` reading is a
/// device reporting garbage, not a device staying quiet.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue<T> {
    Present(T),
    Missing,
    Invalid(String),
}

impl<T> FieldValue<T> {
    pub const fn present(value: T) -> Self {
        Self::Present(value)
    }

    pub const fn missing() -> Self {
        Self::Missing
    }

    pub fn invalid(detail: impl Into<String>) -> Self {
        Self::Invalid(detail.into())
    }

    pub fn from_option(value: Option<T>) -> Self {
        value.map_or(Self::Missing, Self::Present)
    }

    pub fn from_nested_option(value: Option<Option<T>>) -> Self {
        Self::from_option(value.flatten())
    }

    pub fn from_result<E>(value: Result<T, E>) -> Self
    where
        E: fmt::Display,
    {
        match value {
            Ok(value) => Self::Present(value),
            Err(error) => Self::Invalid(error.to_string()),
        }
    }
}

/// Collects the reasons a projection could not produce output.
///
/// A projection offers every required field to [`require`](Self::require)
/// before deciding whether it can build, so a failure reports all of them
/// rather than only the first one read. Reading a field and judging it are
/// separate steps for that reason: the judgement is recorded here and the
/// projection carries on.
#[derive(Debug, Default)]
pub struct Fields {
    issues: Vec<ProjectionIssue>,
}

impl Fields {
    pub fn new() -> Self {
        Self::default()
    }

    /// Yields the value, recording why it was unusable when it is not.
    ///
    /// `path` names the source field in its own schema's terms, so a consumer
    /// reading the issue can look it up against the device's documentation.
    pub fn require<T>(&mut self, path: &'static str, value: FieldValue<T>) -> Option<T> {
        match value {
            FieldValue::Present(value) => Some(value),
            FieldValue::Missing => {
                self.issues.push(ProjectionIssue::new(
                    path,
                    ProjectionIssueKind::MissingRequired,
                ));
                None
            }
            FieldValue::Invalid(detail) => {
                self.issues.push(ProjectionIssue::new(
                    path,
                    ProjectionIssueKind::Invalid { detail },
                ));
                None
            }
        }
    }

    /// Finishes with the projected value, which may still carry issues from
    /// fields that were not required.
    pub fn complete<T>(self, output: T) -> ProjectionResult<T> {
        ProjectionResult::new(Some(output), self.issues)
    }

    /// Finishes with no value, which is only reachable once a required field
    /// has failed.
    pub fn incomplete<T>(self) -> ProjectionResult<T> {
        debug_assert!(
            !self.issues.is_empty(),
            "a projection produced no output and no reason"
        );
        ProjectionResult::new(None, self.issues)
    }
}

#[cfg(test)]
mod tests {
    use super::FieldValue;
    use super::Fields;
    use super::Project;
    use super::ProjectionIssueKind;
    use super::ProjectionResult;

    #[derive(Debug)]
    struct Source {
        name: Option<String>,
        value: Option<u64>,
    }

    #[derive(Debug)]
    struct ExampleProjection;

    impl Project<Source> for ExampleProjection {
        type Output = (String, u64);

        fn project(source: &Source, _context: &()) -> ProjectionResult<Self::Output> {
            let mut fields = Fields::new();
            let name = fields.require("Source.Name", FieldValue::from_option(source.name.clone()));
            let value = fields.require("Source.Value", FieldValue::from_option(source.value));

            let (Some(name), Some(value)) = (name, value) else {
                return fields.incomplete();
            };
            fields.complete((name, value))
        }
    }

    #[test]
    fn projection_collects_all_required_field_issues() {
        let result = ExampleProjection::project(
            &Source {
                name: None,
                value: None,
            },
            &(),
        );

        assert!(result.output().is_none());
        assert_eq!(result.issues().len(), 2);
        assert_eq!(result.issues()[0].path(), "Source.Name");
        assert_eq!(
            result.issues()[0].kind(),
            &ProjectionIssueKind::MissingRequired
        );
    }

    #[test]
    fn projection_builds_output_when_required_fields_are_present() {
        let result = ExampleProjection::project(
            &Source {
                name: Some("sensor".to_owned()),
                value: Some(42),
            },
            &(),
        );

        assert_eq!(result.output(), Some(&("sensor".to_owned(), 42)));
        assert!(result.issues().is_empty());
    }
}
