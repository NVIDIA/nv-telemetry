// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

mod source;
mod threshold;
mod vocabulary;

use nv_redfish::core::EntityTypeRef;
use nv_redfish::schema::sensor::Sensor;
use nv_telemetry_core::Finite;
use nv_telemetry_core::Health;
use nv_telemetry_core::Name;
use nv_telemetry_core::ObservedResource;
use nv_telemetry_core::OperatingState;
use nv_telemetry_core::Property;
use nv_telemetry_core::PropertyMap;
use nv_telemetry_core::PropertyValue;
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
use self::threshold::project_threshold_properties;
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

/// Diagnostic path for a contradiction between the two readable-range bounds.
///
/// An inverted range is a disagreement between two properties, so the path
/// names both, following [`ProjectionIssue`](crate::ProjectionIssue)'s
/// convention for an issue about more than one field.
const RANGE_ORDER: &str = "Sensor.ReadingRangeMin,Sensor.ReadingRangeMax";

/// When the Sensor representation was read.
///
/// Metadata and resource projections stamp this on their output. A sample does
/// not: it carries `ReadingTime` when the device reports one and otherwise
/// leaves its timestamp unset, so a consumer reads the acquisition batch's own
/// observation window rather than a poll time dressed up as a reading time.
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

        let bounds = project_bounds(sensor, &mut fields);

        let (Some(location), Some(id), Some(semantics)) = (location, id, semantics) else {
            return fields.incomplete();
        };

        let subject = location.sensor_subject(&id);
        let source_key = location.into_source_key();
        let instance = id.into_instance();
        let (metric, kind) = semantics.into_parts();
        let unit = sensor
            .reading_units
            .as_ref()
            .and_then(Option::as_deref)
            .filter(|unit| !unit.is_empty());

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
        let value = fields.require(
            "Sensor.Reading",
            project_optional::<_, Finite>(sensor.reading.flatten()),
        );
        let observed_at = fields.optional(
            "Sensor.ReadingTime",
            project_optional::<_, Timestamp>(sensor.reading_time.flatten()),
        );

        let reported_state = project_reported_state(sensor, &mut fields);

        let (Some(source), Some(value)) = (source, value) else {
            return fields.incomplete();
        };

        let mut sample = SignalSample::from_canonical_source(source.into_source_key(), value);
        if let Some(observed_at) = observed_at {
            sample = sample.with_observed_at(observed_at);
        }
        if let Some(reported_state) = reported_state {
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
        // The projected names are distinct literals nesting at most two
        // levels, so a rejected map is this projection's mistake rather than
        // something the device sent, and names the map it broke the way a
        // rejected threshold sub-map does.
        let properties = PropertyMap::new(project_sensor_properties(sensor, &mut fields))
            .inspect_err(|_| {
                fields.invalid_projection(
                    "Sensor",
                    "projected properties do not form a property map",
                );
            })
            .ok();

        let (Some(location), Some(id), Some(properties)) = (location, id, properties) else {
            return fields.incomplete();
        };

        let subject = location.sensor_subject(&id);
        let parent = location.parent_subject();
        let source_key = location.into_source_key();
        let mut resource = ObservedResource::partial(subject.clone(), source_key, properties)
            .with_schema(SCHEMA)
            .with_observed_at(context.observed_at);
        let version = sensor
            .etag()
            .map(ToString::to_string)
            .filter(|etag| !etag.is_empty());
        if let Some(version) = version {
            resource = resource.with_version(Name::from(version));
        }
        fields.complete(SensorResourceRecord {
            resource,
            parent_relation: ResourceRelation::new(parent, CONTAINS, subject),
        })
    }
}

/// Names the state lifted onto the resource, in model vocabulary.
///
/// A device that omits `Thresholds` entirely does not implement them, and gets
/// no threshold properties.
fn project_sensor_properties(sensor: &Sensor, fields: &mut Fields) -> Vec<Property> {
    let mut properties = vec![Property::new(
        Name::from_static("name"),
        PropertyValue::String(sensor.base.name.as_str().into()),
    )];
    if let Some(thresholds) = sensor.thresholds.as_ref() {
        properties.extend(project_threshold_properties(thresholds, fields));
    }
    properties
}

fn project_bounds(sensor: &Sensor, fields: &mut Fields) -> Option<ValueRange> {
    let lower = fields.optional(
        "Sensor.ReadingRangeMin",
        project_optional::<_, Finite>(sensor.reading_range_min.flatten()),
    );
    let upper = fields.optional(
        "Sensor.ReadingRangeMax",
        project_optional::<_, Finite>(sensor.reading_range_max.flatten()),
    );
    let range = fields.optional(
        RANGE_ORDER,
        FieldValue::from_result(match (lower, upper) {
            (Some(lower), Some(upper)) => ValueRange::between(lower, upper),
            (Some(lower), None) => Ok(ValueRange::at_least(lower)),
            (None, Some(upper)) => Ok(ValueRange::at_most(upper)),
            (None, None) => Ok(ValueRange::empty()),
        }),
    )?;
    (!range.is_empty()).then_some(range)
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
