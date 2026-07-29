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

/// Intermediate value consumed by `telemetry_projection!`.
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

    #[doc(hidden)]
    pub fn issue_kind(&self) -> Option<ProjectionIssueKind> {
        match self {
            Self::Present(_) => None,
            Self::Missing => Some(ProjectionIssueKind::MissingRequired),
            Self::Invalid(detail) => Some(ProjectionIssueKind::Invalid {
                detail: detail.clone(),
            }),
        }
    }
}

/// Defines a compile-checked projection from typed source expressions.
///
/// Required expressions return [`FieldValue`]. All required fields are
/// evaluated so one failed projection reports every missing or invalid field.
/// Optional expressions are evaluated only when all required fields are valid.
///
/// Type mismatches in a mapping fail at compile time:
///
/// ```compile_fail
/// use nv_telemetry_redfish::{telemetry_projection, FieldValue};
///
/// struct Source {
///     value: Option<String>,
/// }
///
/// telemetry_projection! {
///     BrokenProjection(Source, ()) -> u64
///     |source, _context| {
///         required {
///             value: "Source.Value" => FieldValue::from_option(source.value.clone())
///         }
///         optional {
///         }
///         build {
///             value
///         }
///     }
/// }
/// ```
#[macro_export]
macro_rules! telemetry_projection {
    (
        $(#[$meta:meta])*
        $vis:vis $name:ident($input:ty, $context:ty) -> $output:ty
        |$source:ident, $ctx:ident| {
            required {
                $(
                    $required:ident : $path:literal => $required_expr:expr
                ),+ $(,)?
            }
            optional {
                $(
                    $optional:ident = $optional_expr:expr
                ),* $(,)?
            }
            build $build:block
        }
    ) => {
        $(#[$meta])*
        // A projection is a type-level tag, never constructed.
        #[derive(::std::fmt::Debug)]
        #[non_exhaustive]
        $vis struct $name;

        impl $crate::Project<$input, $context> for $name {
            type Output = $output;

            fn project(
                $source: &$input,
                $ctx: &$context,
            ) -> $crate::ProjectionResult<Self::Output> {
                let mut issues = ::std::vec::Vec::new();

                $(
                    let $required = $required_expr;
                    if let ::std::option::Option::Some(kind) = $required.issue_kind() {
                        issues.push($crate::ProjectionIssue::new($path, kind));
                    }
                )+

                let output = match ($($required,)+) {
                    ($($crate::FieldValue::Present($required),)+) => {
                        $(
                            let $optional = $optional_expr;
                        )*
                        ::std::option::Option::Some($build)
                    }
                    _ => ::std::option::Option::None,
                };

                $crate::ProjectionResult::new(output, issues)
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::FieldValue;
    use super::Project;
    use super::ProjectionIssueKind;

    #[derive(Debug)]
    struct Source {
        name: Option<String>,
        value: Option<u64>,
    }

    crate::telemetry_projection! {
        ExampleProjection(Source, ()) -> (String, u64)
        |source, _context| {
            required {
                name: "Source.Name" => FieldValue::from_option(source.name.clone()),
                value: "Source.Value" => FieldValue::from_option(source.value)
            }
            optional {
            }
            build {
                (name, value)
            }
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
