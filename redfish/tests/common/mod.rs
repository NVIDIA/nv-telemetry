// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Fixture loading and projection shared by the Sensor test binaries and the
//! projection benchmarks.

// Each binary that includes this module uses part of it.
#![allow(dead_code)]

use nv_redfish::schema::sensor::Sensor;
use nv_telemetry_core::ObservedResource;
use nv_telemetry_core::PropertyValue;
use nv_telemetry_core::Timestamp;
use nv_telemetry_redfish::Project;
use nv_telemetry_redfish::SensorProjectionContext;
use serde_json::Value;

/// The Unix second the fixture is taken to have been observed at.
pub(crate) const OBSERVED_AT: i64 = 1_721_000_000;

pub(crate) fn context() -> SensorProjectionContext {
    context_at(OBSERVED_AT)
}

pub(crate) fn context_at(seconds: i64) -> SensorProjectionContext {
    SensorProjectionContext::new(Timestamp::new(seconds, 0).expect("valid timestamp"))
}

pub(crate) fn fixture_json() -> Value {
    serde_json::from_str(include_str!("../fixtures/sensor.json")).expect("valid fixture JSON")
}

pub(crate) fn sensor_from(json: Value) -> Sensor {
    serde_json::from_value(json).expect("the compiled schema accepts the fixture")
}

pub(crate) fn fixture() -> Sensor {
    sensor_from(fixture_json())
}

/// Projects a sensor, rejecting output that arrived alongside any issue.
///
/// A projection that reported an issue is a different outcome from one that
/// did not, so a test asserting on clean output has to see the difference.
pub(crate) fn project<P>(sensor: &Sensor) -> P::Output
where
    P: Project<Sensor, SensorProjectionContext>,
{
    project_at::<P>(sensor, &context())
}

pub(crate) fn project_at<P>(sensor: &Sensor, context: &SensorProjectionContext) -> P::Output
where
    P: Project<Sensor, SensorProjectionContext>,
{
    let result = P::project(sensor, context);
    assert!(
        result.issues().is_empty(),
        "unexpected issues: {:?}",
        result.issues()
    );
    result.into_parts().0.expect("projection output")
}

/// Reads one leaf out of a projected threshold slot.
///
/// A projected slot is always a nested object, so a slot that is absent or
/// flat fails here rather than reading as an absent leaf.
pub(crate) fn threshold_leaf<'a>(
    resource: &'a ObservedResource,
    slot: &str,
    leaf: &str,
) -> Option<&'a PropertyValue> {
    let Some(PropertyValue::Object(properties)) = resource.properties.get(slot) else {
        panic!("threshold slot {slot} is projected as a nested object");
    };
    properties.get(leaf)
}
