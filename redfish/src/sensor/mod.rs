// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

mod source;
mod threshold;
mod vocabulary;

use nv_redfish::core::EdmDateTimeOffset;
use nv_redfish::core::EntityTypeRef;
use nv_redfish::schema::sensor::Sensor;
use nv_telemetry_core::Finite;
use nv_telemetry_core::Health;
use nv_telemetry_core::Name;
use nv_telemetry_core::ObservedResource;
use nv_telemetry_core::OperatingState;
use nv_telemetry_core::PropertyMap;
use nv_telemetry_core::RelationKind;
use nv_telemetry_core::ReportedState;
use nv_telemetry_core::ResourceRelation;
use nv_telemetry_core::SignalDescriptor;
use nv_telemetry_core::Timestamp;
use nv_telemetry_core::Unit;
use nv_telemetry_core::ValueRange;

use self::source::SensorId;
use self::source::SensorLocation;
use self::source::SensorSource;
use self::threshold::project_sensor_properties;
use self::vocabulary::project_optional;
use self::vocabulary::ReadingSemantics;
use crate::FieldValue;
use crate::Fields;
use crate::Project;
use crate::ProjectionResult;
use crate::SignalDescriptorRecord;
use crate::SignalSample;

/// The unit of a sensor that reports a bare number.
///
/// Redfish leaves `ReadingUnits` off dimensionless sensors, but the model
/// requires every signal to name a unit. UCUM, which `ReadingUnits` draws
/// from, spells unity `1`.
const DIMENSIONLESS: Unit = Unit::from_static("1");

/// The source schema every projection here reads from.
const SCHEMA: Name = Name::from_static("Sensor");

/// Parent-to-child relation emitted with every projected Sensor resource.
const CONTAINS: RelationKind = RelationKind::from_static("contains");

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

/// A projected Sensor resource and the parent relation required to place it.
///
/// The relation source is the Chassis or `PowerDistribution` resource and the
/// target is [`resource`](Self::resource). Keeping the pair together prevents
/// callers from accidentally assembling a subject-scoped graph whose sensor
/// is unreachable from its parent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SensorResourceRecord {
    resource: ObservedResource,
    parent_relation: ResourceRelation,
}

impl SensorResourceRecord {
    pub const fn resource(&self) -> &ObservedResource {
        &self.resource
    }

    pub const fn parent_relation(&self) -> &ResourceRelation {
        &self.parent_relation
    }

    pub fn into_parts(self) -> (ObservedResource, ResourceRelation) {
        (self.resource, self.parent_relation)
    }
}

/// Projects stable metadata from an `nv-redfish` Sensor.
#[derive(Debug)]
#[non_exhaustive]
pub struct SensorMetadataProjection;

impl Project<Sensor, SensorProjectionContext> for SensorMetadataProjection {
    type Output = SignalDescriptorRecord;

    fn project(
        sensor: &Sensor,
        context: &SensorProjectionContext,
    ) -> ProjectionResult<Self::Output> {
        let mut fields = Fields::new();
        let location = fields.require("Sensor.@odata.id", SensorLocation::from_sensor(sensor));
        let id = fields.require("Sensor.Id", SensorId::from_sensor(sensor));
        let semantics = fields.require(
            "Sensor.ReadingType",
            project_optional::<_, ReadingSemantics>(sensor.reading_type.flatten()),
        );

        let (Some(location), Some(id), Some(semantics)) = (location, id, semantics) else {
            return fields.incomplete();
        };

        let subject = location.sensor_subject(&id);
        let source_key = location.into_source_key();
        let instance = id.into_instance();
        let (metric, kind) = semantics.into_parts();
        let unit = sensor
            .reading_units
            .clone()
            .flatten()
            .filter(|unit| !unit.is_empty());

        let mut descriptor = SignalDescriptor::new(
            subject,
            metric,
            instance,
            kind,
            unit.map_or(DIMENSIONLESS, Unit::from),
            context.observed_at,
        );
        if let Some(bounds) = project_bounds(sensor, &mut fields) {
            descriptor = descriptor.with_bounds(bounds);
        }
        fields.complete(SignalDescriptorRecord::from_canonical_source(
            source_key, descriptor,
        ))
    }
}

/// Projects a current reading from an `nv-redfish` Sensor.
#[derive(Debug)]
#[non_exhaustive]
pub struct SensorSampleProjection;

impl Project<Sensor, SensorProjectionContext> for SensorSampleProjection {
    type Output = SignalSample;

    fn project(
        sensor: &Sensor,
        _context: &SensorProjectionContext,
    ) -> ProjectionResult<Self::Output> {
        let mut fields = Fields::new();
        let source = fields.require("Sensor.@odata.id", SensorSource::from_sensor(sensor));
        let value = fields.require("Sensor.Reading", finite_value(sensor.reading.flatten()));
        let observed_at = fields.optional(
            "Sensor.ReadingTime",
            timestamp_value(sensor.reading_time.flatten()),
        );

        let (Some(source), Some(value)) = (source, value) else {
            return fields.incomplete();
        };

        let mut sample = SignalSample::from_canonical_source(source.into_source_key(), value);
        if let Some(observed_at) = observed_at {
            sample = sample.with_observed_at(observed_at);
        }
        if let Some(reported_state) = project_reported_state(sensor, &mut fields) {
            sample = sample.with_reported_state(reported_state);
        }
        fields.complete(sample)
    }
}

/// Projects the configuration a Sensor reports about itself.
///
/// Thresholds are the device's own judgement of its readings, so they are
/// observed state rather than part of a signal's definition. A consumer joins
/// them to a reading through the subject both carry.
///
/// The resource is [`partial`] because this lifts a chosen subset of the
/// representation: a property missing here may simply not be projected, which
/// is why an unconfigured threshold is stated as null rather than left out.
///
/// [`partial`]: nv_telemetry_core::ResourceCompleteness::Partial
#[derive(Debug)]
#[non_exhaustive]
pub struct SensorResourceProjection;

impl Project<Sensor, SensorProjectionContext> for SensorResourceProjection {
    type Output = SensorResourceRecord;

    fn project(
        sensor: &Sensor,
        context: &SensorProjectionContext,
    ) -> ProjectionResult<Self::Output> {
        let mut fields = Fields::new();
        let location = fields.require("Sensor.@odata.id", SensorLocation::from_sensor(sensor));
        let id = fields.require("Sensor.Id", SensorId::from_sensor(sensor));
        let properties = project_sensor_properties(sensor, &mut fields).and_then(PropertyMap::new);
        let properties = fields.require("Sensor", FieldValue::from_result(properties));

        let (Some(location), Some(id), Some(properties)) = (location, id, properties) else {
            return fields.incomplete();
        };

        let subject = location.sensor_subject(&id);
        let parent = location.parent_subject();
        let source_key = location.into_source_key();
        let mut resource = ObservedResource::partial(subject.clone(), source_key, properties)
            .with_schema(SCHEMA)
            .with_observed_at(context.observed_at);
        if let Some(etag) = sensor.etag() {
            resource = resource.with_version(Name::from(etag.to_string()));
        }
        fields.complete(SensorResourceRecord {
            resource,
            parent_relation: ResourceRelation::new(parent, CONTAINS, subject),
        })
    }
}

/// Projects an optional numeric leaf without a method-only wrapper type.
fn finite_value(value: Option<f64>) -> FieldValue<Finite> {
    value.map_or_else(FieldValue::missing, |value| {
        FieldValue::from_result(Finite::new(value))
    })
}

fn project_bounds(sensor: &Sensor, fields: &mut Fields) -> Option<ValueRange> {
    let lower = fields.optional(
        "Sensor.ReadingRangeMin",
        finite_value(sensor.reading_range_min.flatten()),
    );
    let upper = fields.optional(
        "Sensor.ReadingRangeMax",
        finite_value(sensor.reading_range_max.flatten()),
    );
    match (lower, upper) {
        (Some(lower), Some(upper)) => fields.optional(
            "Sensor.ReadingRangeMin",
            FieldValue::from_result(ValueRange::between(lower, upper)),
        ),
        (Some(lower), None) => Some(ValueRange::at_least(lower)),
        (None, Some(upper)) => Some(ValueRange::at_most(upper)),
        (None, None) => None,
    }
}

fn project_reported_state(sensor: &Sensor, fields: &mut Fields) -> Option<ReportedState> {
    let status = sensor.status.as_ref()?;
    let state = fields.optional(
        "Sensor.Status.State",
        project_optional::<_, OperatingState>(status.state.flatten()),
    );
    let health = fields.optional(
        "Sensor.Status.Health",
        project_optional::<_, Health>(status.health.flatten()),
    );
    (state.is_some() || health.is_some()).then(|| ReportedState::new(state, health))
}

/// Projects a timestamp leaf directly because no local invariant is added.
fn timestamp_value(value: Option<EdmDateTimeOffset>) -> FieldValue<Timestamp> {
    let Some(value) = value else {
        return FieldValue::missing();
    };
    let value: time::OffsetDateTime = value.into();
    FieldValue::from_result(Timestamp::new(value.unix_timestamp(), value.nanosecond()))
}
