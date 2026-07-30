// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

use nv_redfish::core::EdmDuration;
use nv_redfish::schema::sensor::Sensor;
use nv_redfish::schema::sensor::Threshold;
use nv_redfish::schema::sensor::Thresholds;
use nv_telemetry_core::DurationValue;
use nv_telemetry_core::Property;
use nv_telemetry_core::PropertyMap;
use nv_telemetry_core::PropertyMapError;
use nv_telemetry_core::PropertyValue;

use super::finite_value;
use super::vocabulary::project_optional;
use super::vocabulary::ProjectedThresholdActivation;
use crate::FieldValue;
use crate::Fields;

/// Names the configuration lifted onto the resource, in model vocabulary.
///
/// A device that omits `Thresholds` entirely does not implement them, and
/// gets no threshold properties. One that reports the object implements them,
/// so each threshold it leaves out is stated as null.
pub(super) fn project_sensor_properties(
    sensor: &Sensor,
    fields: &mut Fields,
) -> Result<Vec<Property>, PropertyMapError> {
    let mut properties = vec![Property::new(
        "name",
        PropertyValue::String(sensor.base.name.as_str().into()),
    )];
    if let Some(thresholds) = sensor.thresholds.as_ref() {
        properties.extend(project_threshold_properties(thresholds, fields)?);
    }
    Ok(properties)
}

#[derive(Clone, Copy, Debug)]
struct ThresholdFieldPaths {
    reading: &'static str,
    activation: &'static str,
    dwell_time: &'static str,
    hysteresis_reading: &'static str,
    hysteresis_duration: &'static str,
}

macro_rules! threshold_field_paths {
    ($threshold:literal) => {
        ThresholdFieldPaths {
            reading: concat!($threshold, ".Reading"),
            activation: concat!($threshold, ".Activation"),
            dwell_time: concat!($threshold, ".DwellTime"),
            hysteresis_reading: concat!($threshold, ".HysteresisReading"),
            hysteresis_duration: concat!($threshold, ".HysteresisDuration"),
        }
    };
}

#[derive(Debug)]
struct ThresholdSlot<'a> {
    property_name: &'static str,
    paths: ThresholdFieldPaths,
    threshold: Option<&'a Threshold>,
}

fn project_threshold_properties(
    thresholds: &Thresholds,
    fields: &mut Fields,
) -> Result<Vec<Property>, PropertyMapError> {
    let slots = [
        ThresholdSlot {
            property_name: "upper_caution",
            paths: threshold_field_paths!("Sensor.Thresholds.UpperCaution"),
            threshold: thresholds.upper_caution.as_ref(),
        },
        ThresholdSlot {
            property_name: "upper_critical",
            paths: threshold_field_paths!("Sensor.Thresholds.UpperCritical"),
            threshold: thresholds.upper_critical.as_ref(),
        },
        ThresholdSlot {
            property_name: "upper_fatal",
            paths: threshold_field_paths!("Sensor.Thresholds.UpperFatal"),
            threshold: thresholds.upper_fatal.as_ref(),
        },
        ThresholdSlot {
            property_name: "lower_caution",
            paths: threshold_field_paths!("Sensor.Thresholds.LowerCaution"),
            threshold: thresholds.lower_caution.as_ref(),
        },
        ThresholdSlot {
            property_name: "lower_critical",
            paths: threshold_field_paths!("Sensor.Thresholds.LowerCritical"),
            threshold: thresholds.lower_critical.as_ref(),
        },
        ThresholdSlot {
            property_name: "lower_fatal",
            paths: threshold_field_paths!("Sensor.Thresholds.LowerFatal"),
            threshold: thresholds.lower_fatal.as_ref(),
        },
        ThresholdSlot {
            property_name: "upper_caution_user",
            paths: threshold_field_paths!("Sensor.Thresholds.UpperCautionUser"),
            threshold: thresholds.upper_caution_user.as_ref(),
        },
        ThresholdSlot {
            property_name: "upper_critical_user",
            paths: threshold_field_paths!("Sensor.Thresholds.UpperCriticalUser"),
            threshold: thresholds.upper_critical_user.as_ref(),
        },
        ThresholdSlot {
            property_name: "lower_caution_user",
            paths: threshold_field_paths!("Sensor.Thresholds.LowerCautionUser"),
            threshold: thresholds.lower_caution_user.as_ref(),
        },
        ThresholdSlot {
            property_name: "lower_critical_user",
            paths: threshold_field_paths!("Sensor.Thresholds.LowerCriticalUser"),
            threshold: thresholds.lower_critical_user.as_ref(),
        },
    ];
    slots
        .into_iter()
        .map(|slot| {
            let value = slot
                .threshold
                .map_or(Ok(PropertyValue::Null), |threshold| {
                    ThresholdProjectionContext::new(slot.paths, fields).project(threshold)
                })?;
            Ok(Property::new(slot.property_name, value))
        })
        .collect()
}

/// Carries the matching diagnostic paths and issue collector together while
/// one threshold object is projected.
#[derive(Debug)]
struct ThresholdProjectionContext<'a> {
    paths: ThresholdFieldPaths,
    fields: &'a mut Fields,
}

impl<'a> ThresholdProjectionContext<'a> {
    fn new(paths: ThresholdFieldPaths, fields: &'a mut Fields) -> Self {
        Self { paths, fields }
    }

    fn project(mut self, threshold: &Threshold) -> Result<PropertyValue, PropertyMapError> {
        let mut properties = Vec::with_capacity(5);
        self.add_field(
            &mut properties,
            self.paths.reading,
            "reading",
            finite_value(threshold.reading.flatten()),
            PropertyValue::F64,
        );
        self.add_field(
            &mut properties,
            self.paths.activation,
            "activation",
            project_optional(threshold.activation.flatten()),
            ProjectedThresholdActivation::into_property_value,
        );
        self.add_field(
            &mut properties,
            self.paths.dwell_time,
            "dwell_time",
            duration_value(threshold.dwell_time.flatten()),
            PropertyValue::Duration,
        );
        self.add_field(
            &mut properties,
            self.paths.hysteresis_reading,
            "hysteresis_reading",
            finite_value(threshold.hysteresis_reading.flatten()),
            PropertyValue::F64,
        );
        self.add_field(
            &mut properties,
            self.paths.hysteresis_duration,
            "hysteresis_duration",
            duration_value(threshold.hysteresis_duration.flatten()),
            PropertyValue::Duration,
        );
        PropertyMap::new(properties).map(PropertyValue::Object)
    }

    fn add_field<T>(
        &mut self,
        properties: &mut Vec<Property>,
        path: &'static str,
        property_name: &'static str,
        value: FieldValue<T>,
        into_property_value: impl FnOnce(T) -> PropertyValue,
    ) {
        match value {
            FieldValue::Present(value) => {
                properties.push(Property::new(property_name, into_property_value(value)));
            }
            FieldValue::Missing => {
                properties.push(Property::new(property_name, PropertyValue::Null));
            }
            FieldValue::Invalid(detail) => {
                let _: Option<T> = self.fields.optional(path, FieldValue::invalid(detail));
            }
        }
    }
}

/// Projects an ISO 8601 duration leaf without inventing a wrapper type.
fn duration_value(value: Option<EdmDuration>) -> FieldValue<DurationValue> {
    let Some(value) = value else {
        return FieldValue::missing();
    };
    let duration = match std::time::Duration::try_from(value) {
        Ok(value) => value,
        Err(error) => return FieldValue::invalid(error.to_string()),
    };
    let seconds = match i64::try_from(duration.as_secs()) {
        Ok(value) => value,
        Err(error) => return FieldValue::invalid(error.to_string()),
    };
    FieldValue::from_result(DurationValue::new(seconds, duration.subsec_nanos()))
}
