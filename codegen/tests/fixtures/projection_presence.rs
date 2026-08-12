// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Generated from `sources/redfish/manifests/test.textpb` by `make codegen`. Do not edit.
//!
//! Deterministic, I/O-free projection from decoded source types to
//! validated observation parts plus issues. Every field is evaluated
//! before identity is decided, absence produces no output, and an
//! unusable answer produces an issue beside the parts.

// Generated code holds the line on correctness lints; the pedantic
// group is style advice for humans and is exactly where a clippy
// release breaks a checked-in file that no one edited.
#![allow(clippy::pedantic)]

/// What one `BootOption` document projected to. The provider
/// assembles batches from these; identity failure leaves every
/// collection empty while the issues still name each fault.
#[derive(Debug)]
pub(crate) struct BootOptionParts {
    pub(crate) state_observations: Vec<::nv_telemetry_model::StateObservation>,
    /// The source fields that projected to nothing, and why.
    pub(crate) issues: Vec<::nv_telemetry_source::ProjectionIssue>,
}
/// Projects one `BootOption` document, located at the *requested*
/// URI.
///
/// # Errors
///
/// `Err` is the residual tier only — a builder refusing inputs this
/// function already triaged is a projection bug, and a bug is an
/// operational fact for the status stream rather than device data.
/// Everything a device can cause comes back as issues inside the
/// parts.
pub(crate) fn project_boot_option(
    boot_option: &::nv_redfish::schema::boot_option::BootOption,
    location: &str,
) -> Result<BootOptionParts, ::nv_telemetry_model::Invalid> {
    let mut issues = Vec::new();
    let boot_reference_value = match boot_option.boot_option_reference.clone() {
        Some(value) => {
            if value.len()
                > ::nv_telemetry_model::limits::VALUE_STRING_VALUE_MAX_LEN as usize
            {
                issues
                    .push(
                        ::nv_telemetry_source::ProjectionIssue::invalid(
                            "BootOption.BootOptionReference",
                            format!(
                                "`string_value`: {} bytes long, over the schema's bound of {}",
                                value.len(),
                                ::nv_telemetry_model::limits::VALUE_STRING_VALUE_MAX_LEN
                            ),
                        ),
                    );
                None
            } else {
                Some(::nv_telemetry_model::Value::string(value)?)
            }
        }
        None => {
            issues
                .push(
                    ::nv_telemetry_source::ProjectionIssue::invalid(
                        "BootOption.BootOptionReference",
                        "explicitly null",
                    ),
                );
            None
        }
    };
    let subject_id = {
        let value = boot_option.base.id.clone();
        if value.is_empty() {
            issues
                .push(
                    ::nv_telemetry_source::ProjectionIssue::invalid(
                        "BootOption.Id",
                        "`id`: present but empty",
                    ),
                );
            None
        } else if value.len() > ::nv_telemetry_model::limits::SUBJECT_ID_MAX_LEN as usize
        {
            issues
                .push(
                    ::nv_telemetry_source::ProjectionIssue::invalid(
                        "BootOption.Id",
                        format!(
                            "`id`: {} bytes long, over the schema's bound of {}", value
                            .len(), ::nv_telemetry_model::limits::SUBJECT_ID_MAX_LEN
                        ),
                    ),
                );
            None
        } else {
            Some(value)
        }
    };
    let scope_0 = match boot_option_subject_scope_0(location) {
        Some(value) => {
            if value.is_empty() {
                issues
                    .push(
                        ::nv_telemetry_source::ProjectionIssue::invalid(
                            "uri",
                            "`scope`: present but empty",
                        ),
                    );
                None
            } else if value.len()
                > ::nv_telemetry_model::limits::SUBJECT_SCOPE_MAX_LEN as usize
            {
                issues
                    .push(
                        ::nv_telemetry_source::ProjectionIssue::invalid(
                            "uri",
                            format!(
                                "`scope`: {} bytes long, over the schema's bound of {}",
                                value.len(),
                                ::nv_telemetry_model::limits::SUBJECT_SCOPE_MAX_LEN
                            ),
                        ),
                    );
                None
            } else {
                Some(value)
            }
        }
        None => {
            issues
                .push(
                    ::nv_telemetry_source::ProjectionIssue::invalid(
                        "uri",
                        "requested location has no system segment",
                    ),
                );
            None
        }
    };
    let subject = match (subject_id, scope_0) {
        (Some(subject_id), Some(scope_0)) => {
            match ::nv_telemetry_model::Subject::builder()
                .kind("boot-option")
                .scope(vec![scope_0.to_owned()])
                .id(subject_id)
                .build()
            {
                Ok(subject) => Some(subject),
                Err(error) => {
                    let path = if error.path().starts_with("id") {
                        "BootOption.Id"
                    } else {
                        "uri"
                    };
                    issues
                        .push(
                            ::nv_telemetry_source::ProjectionIssue::invalid(
                                path,
                                error.to_string(),
                            ),
                        );
                    None
                }
            }
        }
        _ => None,
    };
    let Some(subject) = subject else {
        return Ok(BootOptionParts {
            state_observations: Vec::new(),
            issues,
        });
    };
    let mut state_observations = Vec::new();
    if boot_reference_value.is_some() {
        let mut builder = ::nv_telemetry_model::StateObservation::builder();
        builder = builder.subject(subject.clone());
        if let Some(value) = boot_reference_value {
            builder = builder.value(value);
        }
        builder = builder.name("reference");
        state_observations.push(builder.build()?);
    }
    Ok(BootOptionParts {
        state_observations,
        issues,
    })
}
/// Matches `/redfish/v1/Systems/{system}/BootOptions/{id}`, yielding `{system}`: the subject's
/// scope comes from the requested location, never from the
/// payload's own claim about itself.
fn boot_option_subject_scope_0(location: &str) -> Option<&str> {
    let mut segments = crate::uri::canonical(location).strip_prefix('/')?.split('/');
    if segments.next()? != "redfish" {
        return None;
    }
    if segments.next()? != "v1" {
        return None;
    }
    if segments.next()? != "Systems" {
        return None;
    }
    let captured = segments.next().filter(|segment| !segment.is_empty())?;
    if segments.next()? != "BootOptions" {
        return None;
    }
    let _ = segments.next().filter(|segment| !segment.is_empty())?;
    if segments.next().is_some() {
        return None;
    }
    Some(captured)
}
/// What one `Chassis` document projected to. The provider
/// assembles batches from these; identity failure leaves every
/// collection empty while the issues still name each fault.
#[derive(Debug)]
pub(crate) struct ChassisParts {
    pub(crate) state_observations: Vec<::nv_telemetry_model::StateObservation>,
    /// The source fields that projected to nothing, and why.
    pub(crate) issues: Vec<::nv_telemetry_source::ProjectionIssue>,
}
/// Projects one `Chassis` document, located at the *requested*
/// URI.
///
/// # Errors
///
/// `Err` is the residual tier only — a builder refusing inputs this
/// function already triaged is a projection bug, and a bug is an
/// operational fact for the status stream rather than device data.
/// Everything a device can cause comes back as issues inside the
/// parts.
pub(crate) fn project_chassis(
    chassis: &::nv_redfish::schema::chassis::Chassis,
    location: &str,
) -> Result<ChassisParts, ::nv_telemetry_model::Invalid> {
    let mut issues = Vec::new();
    let nested_null_value = {
        enum GeneratedProjectionValue<T> {
            Absent,
            Null,
            Present(T),
        }
        match match match match GeneratedProjectionValue::Present(chassis) {
            GeneratedProjectionValue::Absent => GeneratedProjectionValue::Absent,
            GeneratedProjectionValue::Null => GeneratedProjectionValue::Null,
            GeneratedProjectionValue::Present(value) => {
                match value.doors.as_ref() {
                    Some(value) => GeneratedProjectionValue::Present(value),
                    None => GeneratedProjectionValue::Absent,
                }
            }
        } {
            GeneratedProjectionValue::Absent => GeneratedProjectionValue::Absent,
            GeneratedProjectionValue::Null => GeneratedProjectionValue::Null,
            GeneratedProjectionValue::Present(value) => {
                match value.front.as_ref() {
                    Some(inner) => {
                        match inner.as_ref() {
                            Some(value) => GeneratedProjectionValue::Present(value),
                            None => GeneratedProjectionValue::Null,
                        }
                    }
                    None => GeneratedProjectionValue::Absent,
                }
            }
        } {
            GeneratedProjectionValue::Absent => GeneratedProjectionValue::Absent,
            GeneratedProjectionValue::Null => GeneratedProjectionValue::Null,
            GeneratedProjectionValue::Present(value) => {
                match value.user_label.clone() {
                    Some(value) => GeneratedProjectionValue::Present(value),
                    None => GeneratedProjectionValue::Absent,
                }
            }
        } {
            GeneratedProjectionValue::Present(value) => {
                if value.len()
                    > ::nv_telemetry_model::limits::VALUE_STRING_VALUE_MAX_LEN as usize
                {
                    issues
                        .push(
                            ::nv_telemetry_source::ProjectionIssue::invalid(
                                "Chassis.Doors.Front.UserLabel",
                                format!(
                                    "`string_value`: {} bytes long, over the schema's bound of {}",
                                    value.len(),
                                    ::nv_telemetry_model::limits::VALUE_STRING_VALUE_MAX_LEN
                                ),
                            ),
                        );
                    None
                } else {
                    Some(::nv_telemetry_model::Value::string(value)?)
                }
            }
            GeneratedProjectionValue::Null => {
                issues
                    .push(
                        ::nv_telemetry_source::ProjectionIssue::invalid(
                            "Chassis.Doors.Front.UserLabel",
                            "explicitly null",
                        ),
                    );
                None
            }
            GeneratedProjectionValue::Absent => None,
        }
    };
    let subject_id = {
        let value = chassis.base.id.clone();
        if value.is_empty() {
            issues
                .push(
                    ::nv_telemetry_source::ProjectionIssue::invalid(
                        "Chassis.Id",
                        "`id`: present but empty",
                    ),
                );
            None
        } else if value.len() > ::nv_telemetry_model::limits::SUBJECT_ID_MAX_LEN as usize
        {
            issues
                .push(
                    ::nv_telemetry_source::ProjectionIssue::invalid(
                        "Chassis.Id",
                        format!(
                            "`id`: {} bytes long, over the schema's bound of {}", value
                            .len(), ::nv_telemetry_model::limits::SUBJECT_ID_MAX_LEN
                        ),
                    ),
                );
            None
        } else {
            Some(value)
        }
    };
    let scope_0 = match chassis_subject_scope_0(location) {
        Some(value) => {
            if value.is_empty() {
                issues
                    .push(
                        ::nv_telemetry_source::ProjectionIssue::invalid(
                            "uri",
                            "`scope`: present but empty",
                        ),
                    );
                None
            } else if value.len()
                > ::nv_telemetry_model::limits::SUBJECT_SCOPE_MAX_LEN as usize
            {
                issues
                    .push(
                        ::nv_telemetry_source::ProjectionIssue::invalid(
                            "uri",
                            format!(
                                "`scope`: {} bytes long, over the schema's bound of {}",
                                value.len(),
                                ::nv_telemetry_model::limits::SUBJECT_SCOPE_MAX_LEN
                            ),
                        ),
                    );
                None
            } else {
                Some(value)
            }
        }
        None => {
            issues
                .push(
                    ::nv_telemetry_source::ProjectionIssue::invalid(
                        "uri",
                        "requested location has no chassis segment",
                    ),
                );
            None
        }
    };
    let subject = match (subject_id, scope_0) {
        (Some(subject_id), Some(scope_0)) => {
            match ::nv_telemetry_model::Subject::builder()
                .kind("chassis")
                .scope(vec![scope_0.to_owned()])
                .id(subject_id)
                .build()
            {
                Ok(subject) => Some(subject),
                Err(error) => {
                    let path = if error.path().starts_with("id") {
                        "Chassis.Id"
                    } else {
                        "uri"
                    };
                    issues
                        .push(
                            ::nv_telemetry_source::ProjectionIssue::invalid(
                                path,
                                error.to_string(),
                            ),
                        );
                    None
                }
            }
        }
        _ => None,
    };
    let Some(subject) = subject else {
        return Ok(ChassisParts {
            state_observations: Vec::new(),
            issues,
        });
    };
    let mut state_observations = Vec::new();
    if nested_null_value.is_some() {
        let mut builder = ::nv_telemetry_model::StateObservation::builder();
        builder = builder.subject(subject.clone());
        if let Some(value) = nested_null_value {
            builder = builder.value(value);
        }
        builder = builder.name("nested-null");
        state_observations.push(builder.build()?);
    }
    Ok(ChassisParts {
        state_observations,
        issues,
    })
}
/// Matches `/redfish/v1/Chassis/{chassis}`, yielding `{chassis}`: the subject's
/// scope comes from the requested location, never from the
/// payload's own claim about itself.
fn chassis_subject_scope_0(location: &str) -> Option<&str> {
    let mut segments = crate::uri::canonical(location).strip_prefix('/')?.split('/');
    if segments.next()? != "redfish" {
        return None;
    }
    if segments.next()? != "v1" {
        return None;
    }
    if segments.next()? != "Chassis" {
        return None;
    }
    let captured = segments.next().filter(|segment| !segment.is_empty())?;
    if segments.next().is_some() {
        return None;
    }
    Some(captured)
}
