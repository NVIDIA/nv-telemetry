// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

use nv_redfish::core::EntityTypeRef;
use nv_redfish::core::ToSnakeCase;
use nv_redfish::schema::sensor::ReadingType;
use nv_redfish::schema::sensor::Sensor;
use nv_redfish::schema::sensor::Thresholds;
use nv_telemetry_core::Finite;
use nv_telemetry_core::Name;
use nv_telemetry_core::ObservedResource;
use nv_telemetry_core::Property;
use nv_telemetry_core::PropertyMap;
use nv_telemetry_core::PropertyValue;
use nv_telemetry_core::ReadingKind;
use nv_telemetry_core::ReportedState;
use nv_telemetry_core::SignalDescriptor;
use nv_telemetry_core::Subject;
use nv_telemetry_core::Timestamp;
use nv_telemetry_core::Unit;
use nv_telemetry_core::ValueRange;

use crate::uri::chassis_of;
use crate::FieldValue;
use crate::SignalDescriptorRecord;
use crate::SignalSample;

/// The unit of a sensor that reports a bare number.
///
/// Redfish leaves `ReadingUnits` off dimensionless sensors, but the model
/// requires every signal to name a unit. UCUM, which `ReadingUnits` draws
/// from, spells unity `1`.
const DIMENSIONLESS: Unit = Unit::from_static("1");

/// The subject kind every sensor observation is filed under.
const SENSOR: Name = Name::from_static("sensor");

/// The source schema every projection here reads from.
const SCHEMA: Name = Name::from_static("Sensor");

/// Context shared by Sensor metadata and sample projections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SensorProjectionContext {
    observed_at: Timestamp,
}

impl SensorProjectionContext {
    pub const fn new(observed_at: Timestamp) -> Self {
        Self { observed_at }
    }

    pub const fn observed_at(self) -> Timestamp {
        self.observed_at
    }
}

crate::telemetry_projection! {
    /// Projects stable metadata from an `nv-redfish` Sensor.
    pub SensorMetadataProjection(Sensor, SensorProjectionContext) -> SignalDescriptorRecord
    |sensor, context| {
        required {
            subject: "Sensor.@odata.id" => sensor_subject(sensor),
            source_key: "Sensor.@odata.id" =>
                FieldValue::present(Name::from(sensor.odata_id().to_string())),
            instance: "Sensor.Id" =>
                FieldValue::present(Name::from(sensor.base.id.as_str())),
            metric: "Sensor.ReadingType" =>
                FieldValue::from_option(
                    sensor.reading_type.flatten().map(|value| Name::from_static(value.to_snake_case()))
                )
        }
        optional {
            kind = sensor.reading_type.flatten().map_or(ReadingKind::Gauge, reading_kind),
            unit = sensor.reading_units.clone().flatten().filter(|unit| !unit.is_empty()),
            bounds = project_bounds(sensor)
        }
        build {
            let mut descriptor = SignalDescriptor::new(
                subject,
                metric,
                instance,
                kind,
                unit.map_or(DIMENSIONLESS, Unit::from),
                context.observed_at,
            );
            if let Some(bounds) = bounds {
                descriptor = descriptor.with_bounds(bounds);
            }
            SignalDescriptorRecord::new(source_key, descriptor)
        }
    }
}

crate::telemetry_projection! {
    /// Projects a current reading from an `nv-redfish` Sensor.
    pub SensorSampleProjection(Sensor, SensorProjectionContext) -> SignalSample
    |sensor, context| {
        required {
            source_key: "Sensor.@odata.id" =>
                FieldValue::present(Name::from(sensor.odata_id().to_string())),
            value: "Sensor.Reading" => reading_value(sensor.reading.flatten())
        }
        optional {
            reported_state = project_reported_state(sensor)
        }
        build {
            let mut sample = SignalSample::new(source_key.clone(), source_key, value)
                .with_observed_at(context.observed_at);
            if let Some(reported_state) = reported_state {
                sample = sample.with_reported_state(reported_state);
            }
            sample
        }
    }
}

crate::telemetry_projection! {
    /// Projects the configuration a Sensor reports about itself.
    ///
    /// Thresholds are the device's own judgement of its readings, so they are
    /// observed state rather than part of a signal's definition. A consumer
    /// joins them to a reading through the subject both carry.
    ///
    /// The resource is [`partial`] because this lifts a chosen subset of the
    /// representation: a property missing here may simply not be projected,
    /// which is why an unconfigured threshold is stated as null rather than
    /// left out.
    ///
    /// [`partial`]: nv_telemetry_core::ResourceCompleteness::Partial
    pub SensorResourceProjection(Sensor, SensorProjectionContext) -> ObservedResource
    |sensor, context| {
        required {
            subject: "Sensor.@odata.id" => sensor_subject(sensor),
            source_key: "Sensor.@odata.id" =>
                FieldValue::present(Name::from(sensor.odata_id().to_string())),
            properties: "Sensor.Thresholds" =>
                FieldValue::from_result(PropertyMap::new(sensor_properties(sensor)))
        }
        optional {
        }
        build {
            ObservedResource::partial(subject, source_key, properties)
                .with_schema(SCHEMA)
                .with_observed_at(context.observed_at)
        }
    }
}

/// Separates the readings that accumulate from the ones that stand alone.
///
/// Redfish defines the energy and charge types as an integral over time that
/// resets, which is a counter. Everything else reports the value at the
/// instant it was read. A consumer averages a gauge but differences a counter,
/// so the distinction has to survive projection.
fn reading_kind(reading_type: ReadingType) -> ReadingKind {
    match reading_type {
        ReadingType::EnergykWh
        | ReadingType::EnergyJoules
        | ReadingType::EnergyWh
        | ReadingType::ChargeAh => ReadingKind::Counter,
        _ => ReadingKind::Gauge,
    }
}

/// Scopes a sensor's identity by the chassis holding it.
fn sensor_subject(sensor: &Sensor) -> FieldValue<Subject> {
    if sensor.base.id.is_empty() {
        return FieldValue::invalid("Sensor.Id is empty");
    }
    let uri = sensor.odata_id().to_string();
    match chassis_of(&uri) {
        Some(chassis) => FieldValue::present(Subject::new(
            SENSOR,
            format!("{chassis}/{}", sensor.base.id),
        )),
        None => FieldValue::invalid(format!("no Chassis segment in {uri}")),
    }
}

/// Reports a reading the device sent but the model cannot hold.
fn reading_value(value: Option<f64>) -> FieldValue<Finite> {
    match value {
        Some(value) => FieldValue::from_result(Finite::new(value)),
        None => FieldValue::missing(),
    }
}

/// Discards a non-finite bound, which constrains nothing.
fn finite_reading(value: Option<f64>) -> Option<Finite> {
    value.and_then(|value| Finite::new(value).ok())
}

fn project_bounds(sensor: &Sensor) -> Option<ValueRange> {
    let bounds = ValueRange::new(
        finite_reading(sensor.reading_range_min.flatten()),
        finite_reading(sensor.reading_range_max.flatten()),
    );
    (!bounds.is_empty())
        .then_some(bounds)
        .and_then(|bounds| bounds.checked().ok())
}

fn project_reported_state(sensor: &Sensor) -> Option<ReportedState> {
    let status = sensor.status.as_ref()?;
    let state = status
        .state
        .flatten()
        .map(|value| Name::from_static(value.to_snake_case()));
    let health = status
        .health
        .flatten()
        .map(|value| Name::from_static(value.to_snake_case()));
    (state.is_some() || health.is_some()).then(|| ReportedState::new(state, health))
}

/// Names the configuration lifted onto the resource, in model vocabulary.
///
/// A device that omits `Thresholds` entirely does not implement them, and
/// gets no threshold properties. One that reports the object implements them,
/// so each threshold it leaves out is stated as null.
fn sensor_properties(sensor: &Sensor) -> Vec<Property> {
    let mut properties = vec![Property::new(
        "name",
        PropertyValue::String(sensor.base.name.as_str().into()),
    )];
    if let Some(thresholds) = sensor.thresholds.as_ref() {
        properties.extend(threshold_properties(thresholds));
    }
    properties
}

fn threshold_properties(thresholds: &Thresholds) -> impl Iterator<Item = Property> + '_ {
    [
        ("upper_caution", &thresholds.upper_caution),
        ("upper_critical", &thresholds.upper_critical),
        ("upper_fatal", &thresholds.upper_fatal),
        ("lower_caution", &thresholds.lower_caution),
        ("lower_critical", &thresholds.lower_critical),
        ("lower_fatal", &thresholds.lower_fatal),
    ]
    .into_iter()
    .map(|(name, threshold)| {
        let reading = threshold
            .as_ref()
            .and_then(|threshold| finite_reading(threshold.reading.flatten()));
        Property::new(
            name,
            reading.map_or(PropertyValue::Null, PropertyValue::F64),
        )
    })
}
