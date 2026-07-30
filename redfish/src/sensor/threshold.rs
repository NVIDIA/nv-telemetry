// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

use nv_redfish::schema::sensor::Threshold;
use nv_redfish::schema::sensor::Thresholds;
use nv_telemetry_core::Name;
use nv_telemetry_core::Property;
use nv_telemetry_core::PropertyMap;
use nv_telemetry_core::PropertyValue;

use super::vocabulary::project_optional;
use super::vocabulary::ProjectedThresholdActivation;
use crate::FieldValue;
use crate::Fields;

/// Names the thresholds lifted onto the resource, in model vocabulary.
///
/// A device that reports the `Thresholds` object implements thresholds, so
/// each slot it leaves out is stated as null.
pub(super) fn project_threshold_properties(
    thresholds: &Thresholds,
    fields: &mut Fields,
) -> Vec<Property> {
    let slots = [
        ("upper_caution", "UpperCaution", &thresholds.upper_caution),
        (
            "upper_critical",
            "UpperCritical",
            &thresholds.upper_critical,
        ),
        ("upper_fatal", "UpperFatal", &thresholds.upper_fatal),
        ("lower_caution", "LowerCaution", &thresholds.lower_caution),
        (
            "lower_critical",
            "LowerCritical",
            &thresholds.lower_critical,
        ),
        ("lower_fatal", "LowerFatal", &thresholds.lower_fatal),
        (
            "upper_caution_user",
            "UpperCautionUser",
            &thresholds.upper_caution_user,
        ),
        (
            "upper_critical_user",
            "UpperCriticalUser",
            &thresholds.upper_critical_user,
        ),
        (
            "lower_caution_user",
            "LowerCautionUser",
            &thresholds.lower_caution_user,
        ),
        (
            "lower_critical_user",
            "LowerCriticalUser",
            &thresholds.lower_critical_user,
        ),
    ];
    slots
        .into_iter()
        .filter_map(|(property_name, redfish_name, threshold)| {
            let value = match threshold.as_ref() {
                Some(threshold) => {
                    ThresholdProjection::new(redfish_name, fields).project(threshold)?
                }
                None => PropertyValue::Null,
            };
            Some(Property::new(Name::from_static(property_name), value))
        })
        .collect()
}

/// Carries the Redfish object under projection and the issue collector while
/// one threshold's leaves are read.
///
/// The object's name composes the diagnostic path of every leaf, so a leaf
/// cannot be reported against the wrong slot.
#[derive(Debug)]
struct ThresholdProjection<'a> {
    redfish_name: &'static str,
    fields: &'a mut Fields,
}

impl<'a> ThresholdProjection<'a> {
    fn new(redfish_name: &'static str, fields: &'a mut Fields) -> Self {
        Self {
            redfish_name,
            fields,
        }
    }

    /// Projects one threshold object into a nested property value.
    ///
    /// The five leaf names are distinct literals nesting one level, so a
    /// rejected map is a mistake in this function rather than something the
    /// device sent, and is reported as one before the slot is dropped.
    fn project(mut self, threshold: &Threshold) -> Option<PropertyValue> {
        let mut properties = Vec::with_capacity(5);
        self.add_field(
            &mut properties,
            "Reading",
            "reading",
            project_optional(threshold.reading.flatten()),
            PropertyValue::F64,
        );
        self.add_field(
            &mut properties,
            "Activation",
            "activation",
            project_optional(threshold.activation.flatten()),
            ProjectedThresholdActivation::into_property_value,
        );
        self.add_field(
            &mut properties,
            "DwellTime",
            "dwell_time",
            project_optional(threshold.dwell_time.flatten()),
            PropertyValue::Duration,
        );
        self.add_field(
            &mut properties,
            "HysteresisReading",
            "hysteresis_reading",
            project_optional(threshold.hysteresis_reading.flatten()),
            PropertyValue::F64,
        );
        self.add_field(
            &mut properties,
            "HysteresisDuration",
            "hysteresis_duration",
            project_optional(threshold.hysteresis_duration.flatten()),
            PropertyValue::Duration,
        );
        let Ok(properties) = PropertyMap::new(properties) else {
            self.fields.invalid_projection(
                format!("Sensor.Thresholds.{}", self.redfish_name),
                "threshold leaves do not form a property map",
            );
            return None;
        };
        Some(PropertyValue::Object(properties))
    }

    /// Reads one leaf into the slot, recording an unusable one as an issue.
    ///
    /// A leaf the device left unset is stated as null, which is the claim that
    /// it configures nothing there. A leaf it sent unusably is left out
    /// instead: absence in a [`partial`] resource carries no information, and
    /// no information is what a rejected value leaves. Stating it as null
    /// would put a claim the device never made in front of a consumer that
    /// keeps the resource and not the issues, which would then read a rejected
    /// critical threshold as an unconfigured one.
    ///
    /// [`partial`]: nv_telemetry_core::ResourceCompleteness::Partial
    fn add_field<T>(
        &mut self,
        properties: &mut Vec<Property>,
        source_name: &str,
        property_name: &'static str,
        value: FieldValue<T>,
        into_property_value: impl FnOnce(T) -> PropertyValue,
    ) {
        let value = match value {
            FieldValue::Present(value) => into_property_value(value),
            FieldValue::Missing => PropertyValue::Null,
            FieldValue::Invalid(detail) => {
                self.fields.invalid(
                    format!("Sensor.Thresholds.{}.{source_name}", self.redfish_name),
                    detail,
                );
                return;
            }
        };
        properties.push(Property::new(Name::from_static(property_name), value));
    }
}
