// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

use std::borrow::Cow;
use std::fmt;

/// Converts one typed source value into one telemetry value.
///
/// `Project` is a static interface: implementations are normally zero-sized
/// marker types selected through a generic parameter, and projection is
/// dispatched at compile time. The associated function deliberately does not
/// take `self`, so this trait is not a runtime trait-object interface. Code
/// that needs runtime selection can store typed function pointers such as
/// `P::project` or wrap implementations in an object-safe adapter.
pub trait Project<Input, Context = ()> {
    type Output;

    fn project(input: &Input, context: &Context) -> ProjectionResult<Self::Output>;
}

/// Why a projection field could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProjectionIssueKind {
    MissingRequired,
    Invalid {
        detail: String,
    },
    /// A projection implementation finished in a state inconsistent with the
    /// required fields it had evaluated.
    InvalidProjection {
        detail: &'static str,
    },
}

impl fmt::Display for ProjectionIssueKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRequired => formatter.write_str("required field is missing"),
            Self::Invalid { detail } => write!(formatter, "invalid field: {detail}"),
            Self::InvalidProjection { detail } => {
                write!(formatter, "invalid projection: {detail}")
            }
        }
    }
}

/// A source field that was missing or unusable during projection.
///
/// A path is a source-schema location, so it is normally a static literal. It
/// is owned only where a projection composes one from a repeated source
/// structure, such as a threshold slot's five leaves.
///
/// A path names every field the issue is about, comma-separated, which is more
/// than one where two properties contradict each other and neither alone is
/// the field a consumer should go and read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectionIssue {
    path: Cow<'static, str>,
    kind: ProjectionIssueKind,
}

impl ProjectionIssue {
    pub fn new(path: impl Into<Cow<'static, str>>, kind: ProjectionIssueKind) -> Self {
        Self {
            path: path.into(),
            kind,
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub const fn kind(&self) -> &ProjectionIssueKind {
        &self.kind
    }
}

impl fmt::Display for ProjectionIssue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.path, self.kind)
    }
}

/// Result of projecting one source value.
///
/// A result without output always has at least one required-field or invalid
/// projection issue. A result with output may have issues from invalid
/// optional fields. [`Fields`] is the only public way to construct a result so
/// those states cannot be confused.
#[must_use = "projection issues and output must be handled"]
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectionResult<T> {
    output: Option<T>,
    issues: Box<[ProjectionIssue]>,
}

impl<T> ProjectionResult<T> {
    fn new(output: Option<T>, issues: Vec<ProjectionIssue>) -> Self {
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
#[must_use = "discarding a field value discards why the source field was unusable"]
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
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

    pub fn from_result<E>(value: Result<T, E>) -> Self
    where
        E: fmt::Display,
    {
        match value {
            Ok(value) => Self::Present(value),
            Err(error) => Self::Invalid(error.to_string()),
        }
    }

    /// Retypes a present value, preserving why an absent one is absent.
    pub fn map<U>(self, transform: impl FnOnce(T) -> U) -> FieldValue<U> {
        self.and_then(|value| FieldValue::Present(transform(value)))
    }

    /// Continues judging a present value, preserving why an absent one is
    /// absent.
    pub fn and_then<U>(self, transform: impl FnOnce(T) -> FieldValue<U>) -> FieldValue<U> {
        match self {
            Self::Present(value) => transform(value),
            Self::Missing => FieldValue::Missing,
            Self::Invalid(detail) => FieldValue::Invalid(detail),
        }
    }
}

/// Collects field issues and tracks whether projection can produce output.
///
/// A projection offers every required field to [`require`](Self::require)
/// before deciding whether it can build, so a failure reports all of them
/// rather than only the first one read. Reading a field and judging it are
/// separate steps for that reason: the judgement is recorded here and the
/// projection carries on.
#[derive(Debug, Default)]
pub struct Fields {
    issues: Vec<ProjectionIssue>,
    required_failures: usize,
}

impl Fields {
    pub fn new() -> Self {
        Self::default()
    }

    /// Yields the value, recording why it was unusable when it is not.
    ///
    /// `path` names the source field in its own schema's terms, so a consumer
    /// reading the issue can look it up against the device's documentation.
    pub fn require<T>(
        &mut self,
        path: impl Into<Cow<'static, str>>,
        value: FieldValue<T>,
    ) -> Option<T> {
        match value {
            FieldValue::Present(value) => Some(value),
            FieldValue::Missing => {
                self.issues.push(ProjectionIssue::new(
                    path,
                    ProjectionIssueKind::MissingRequired,
                ));
                self.required_failures += 1;
                None
            }
            FieldValue::Invalid(detail) => {
                self.issues.push(ProjectionIssue::new(
                    path,
                    ProjectionIssueKind::Invalid { detail },
                ));
                self.required_failures += 1;
                None
            }
        }
    }

    /// Yields an optional value and records invalid data without making it a
    /// required-field failure.
    ///
    /// Missing optional fields are expected and produce no issue. Invalid
    /// optional fields are reported so callers can distinguish absent source
    /// data from unusable source data while still receiving projected output.
    pub fn optional<T>(
        &mut self,
        path: impl Into<Cow<'static, str>>,
        value: FieldValue<T>,
    ) -> Option<T> {
        match value {
            FieldValue::Present(value) => Some(value),
            FieldValue::Missing => None,
            FieldValue::Invalid(detail) => {
                self.issues.push(ProjectionIssue::new(
                    path,
                    ProjectionIssueKind::Invalid { detail },
                ));
                None
            }
        }
    }

    /// Records that a source field was unusable without yielding a value for
    /// it.
    ///
    /// [`optional`](Self::optional) reports the same issue while unwrapping the
    /// value, and is what a projection that has a value to unwrap should use.
    /// This is for a projection that has already decided what it will state in
    /// place of the field and only needs the reason recorded.
    pub fn invalid(&mut self, path: impl Into<Cow<'static, str>>, detail: impl Into<String>) {
        self.issues.push(ProjectionIssue::new(
            path,
            ProjectionIssueKind::Invalid {
                detail: detail.into(),
            },
        ));
    }

    /// Records that the projection itself broke an invariant it documents,
    /// without failing a required field.
    ///
    /// A projection that discards part of its own output has to say so, the
    /// same way [`incomplete`](Self::incomplete) does for a whole one. The
    /// detail is a static literal because the projection, not the device,
    /// chooses it.
    pub fn invalid_projection(&mut self, path: impl Into<Cow<'static, str>>, detail: &'static str) {
        self.issues.push(ProjectionIssue::new(
            path,
            ProjectionIssueKind::InvalidProjection { detail },
        ));
    }

    /// Finishes with the projected value and any optional-field issues.
    ///
    /// If a required field failed, the output is discarded and the collected
    /// issues are returned. This keeps malformed device input from turning a
    /// projection implementation mistake into a process panic.
    pub fn complete<T>(self, output: T) -> ProjectionResult<T> {
        let output = (self.required_failures == 0).then_some(output);
        ProjectionResult::new(output, self.issues)
    }

    /// Finishes with no value, which is only reachable once a required field
    /// has failed.
    ///
    /// Calling this without a required-field failure yields an
    /// [`ProjectionIssueKind::InvalidProjection`] issue instead of panicking.
    /// Optional-field issues alone do not justify incomplete output.
    pub fn incomplete<T>(mut self) -> ProjectionResult<T> {
        if self.required_failures == 0 {
            self.issues.push(ProjectionIssue::new(
                "$projection",
                ProjectionIssueKind::InvalidProjection {
                    detail: "produced no output without a required field failure",
                },
            ));
        }
        ProjectionResult::new(None, self.issues)
    }
}

#[cfg(test)]
mod tests {
    use super::FieldValue;
    use super::Fields;
    use super::Project;
    use super::ProjectionIssue;
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

    #[test]
    fn optional_missing_field_is_not_an_issue() {
        let mut fields = Fields::new();
        let value: Option<u64> = fields.optional("Source.Optional", FieldValue::missing());

        assert_eq!(value, None);
        let result = fields.complete("output");
        assert_eq!(result.output(), Some(&"output"));
        assert!(result.issues().is_empty());
    }

    #[test]
    fn optional_invalid_field_is_reported_without_blocking_output() {
        let mut fields = Fields::new();
        let value: Option<u64> = fields.optional(
            "Source.Optional",
            FieldValue::invalid("not a finite number"),
        );

        assert_eq!(value, None);
        let result = fields.complete("output");
        assert_eq!(result.output(), Some(&"output"));
        assert_eq!(result.issues().len(), 1);
        assert_eq!(
            result.issues()[0].kind(),
            &ProjectionIssueKind::Invalid {
                detail: "not a finite number".to_owned(),
            }
        );
    }

    #[test]
    fn required_failure_discards_completed_output() {
        let mut fields = Fields::new();
        let _: Option<u64> = fields.require("Source.Value", FieldValue::missing());

        let result = fields.complete("invalid output");

        assert!(result.output().is_none());
        assert_eq!(result.issues().len(), 1);
        assert_eq!(
            result.issues()[0].kind(),
            &ProjectionIssueKind::MissingRequired
        );
    }

    #[test]
    fn an_invalid_field_can_be_reported_without_a_value_to_unwrap() {
        let mut fields = Fields::new();
        fields.invalid("Source.Optional", "not a finite number");

        let result = fields.complete("output");

        assert_eq!(result.output(), Some(&"output"));
        assert_eq!(result.issues().len(), 1);
        assert_eq!(result.issues()[0].path(), "Source.Optional");
        assert_eq!(
            result.issues()[0].kind(),
            &ProjectionIssueKind::Invalid {
                detail: "not a finite number".to_owned(),
            }
        );
    }

    #[test]
    fn a_partial_discard_is_reported_without_blocking_output() {
        let mut fields = Fields::new();
        fields.invalid_projection("Source.Nested", "nested leaves do not form a map");

        let result = fields.complete("output");

        assert_eq!(result.output(), Some(&"output"));
        assert_eq!(result.issues().len(), 1);
        assert_eq!(
            result.issues()[0].kind(),
            &ProjectionIssueKind::InvalidProjection {
                detail: "nested leaves do not form a map",
            }
        );
    }

    #[test]
    fn incomplete_without_required_failure_reports_projection_issue() {
        let mut fields = Fields::new();
        let _: Option<u64> = fields.optional("Source.Optional", FieldValue::invalid("invalid"));

        let result: ProjectionResult<()> = fields.incomplete();

        assert!(result.output().is_none());
        assert_eq!(result.issues().len(), 2);
        assert_eq!(result.issues()[1].path(), "$projection");
        assert_eq!(
            result.issues()[1].kind(),
            &ProjectionIssueKind::InvalidProjection {
                detail: "produced no output without a required field failure",
            }
        );
    }

    #[test]
    fn projection_issues_have_contextual_display() {
        let missing = ProjectionIssue::new("Source.Name", ProjectionIssueKind::MissingRequired);
        let invalid = ProjectionIssue::new(
            "Source.Value",
            ProjectionIssueKind::Invalid {
                detail: "outside range".to_owned(),
            },
        );

        assert_eq!(
            missing.to_string(),
            "Source.Name: required field is missing"
        );
        assert_eq!(
            invalid.to_string(),
            "Source.Value: invalid field: outside range"
        );
    }

    #[test]
    fn project_can_be_selected_as_a_typed_function_pointer() {
        let project: fn(&Source, &()) -> ProjectionResult<(String, u64)> =
            ExampleProjection::project;
        let result = project(
            &Source {
                name: Some("sensor".to_owned()),
                value: Some(42),
            },
            &(),
        );

        assert_eq!(result.output(), Some(&("sensor".to_owned(), 42)));
    }
}
