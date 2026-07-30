// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

mod common;

use std::sync::Arc;

use common::context;
use common::context_at;
use common::fixture;
use common::project_at;
use common::OBSERVED_AT;
use nv_telemetry_core::Finite;
use nv_telemetry_core::Instance;
use nv_telemetry_core::NumericValue;
use nv_telemetry_core::Timestamp;
use nv_telemetry_redfish::FieldValue;
use nv_telemetry_redfish::Fields;
use nv_telemetry_redfish::Project;
use nv_telemetry_redfish::ProjectionResult;
use nv_telemetry_redfish::SensorMetadataProjection;
use nv_telemetry_redfish::SensorSampleProjection;
use nv_telemetry_redfish::SignalCatalog;
use nv_telemetry_redfish::SignalKey;
use nv_telemetry_redfish::SignalSample;
use nv_telemetry_redfish::SignalUpdate;

#[derive(Debug)]
struct ExternalSource {
    value: Option<u64>,
}

#[derive(Debug)]
struct ExternalProjection;

impl Project<ExternalSource> for ExternalProjection {
    type Output = u64;

    fn project(source: &ExternalSource, _context: &()) -> ProjectionResult<Self::Output> {
        let mut fields = Fields::new();
        let Some(value) = fields.require(
            "ExternalSource.Value",
            FieldValue::from_option(source.value),
        ) else {
            return fields.incomplete();
        };
        fields.complete(value * 2)
    }
}

#[test]
fn project_can_be_extended_outside_the_crate() {
    let result = ExternalProjection::project(&ExternalSource { value: Some(21) }, &());

    assert_eq!(result.output(), Some(&42));
    assert!(result.issues().is_empty());
}

/// `map` changes the field's type, which is why it is not a `FnOnce(T) -> T`:
/// a projection reads what the device wrote and hands on a model value of
/// another type, and an absent field has to cross that change of type still
/// carrying the reason it was absent.
#[test]
fn retyping_a_field_keeps_why_an_absent_one_is_absent() {
    let instance = |index: u64| Instance::from(format!("CPU{index}Temp"));

    assert_eq!(
        FieldValue::present(0_u64).map(instance),
        FieldValue::Present(Instance::from_static("CPU0Temp"))
    );
    assert_eq!(
        FieldValue::<u64>::missing().map(instance),
        FieldValue::Missing
    );
    assert_eq!(
        FieldValue::<u64>::invalid("not a number").map(instance),
        FieldValue::Invalid("not a number".to_owned())
    );
}

#[test]
fn and_then_may_reject_a_present_field_but_never_reinterprets_an_absent_one() {
    let reject = |_: u64| FieldValue::<Instance>::invalid("out of range");

    assert_eq!(
        FieldValue::present(21_u64).and_then(reject),
        FieldValue::Invalid("out of range".to_owned())
    );
    assert_eq!(
        FieldValue::<u64>::missing().and_then(reject),
        FieldValue::Missing
    );
    assert_eq!(
        FieldValue::<u64>::invalid("not a number").and_then(reject),
        FieldValue::Invalid("not a number".to_owned()),
        "a later judgement does not overwrite an earlier failure"
    );
}

#[test]
fn compiled_sensor_metadata_and_sample_join_without_an_adapter() {
    let sensor = fixture();
    let context = context();
    let record = project_at::<SensorMetadataProjection>(&sensor, &context);
    let sample = project_at::<SensorSampleProjection>(&sensor, &context);
    let mut catalog = SignalCatalog::new();
    catalog.upsert(record).expect("catalog capacity");
    let reading = catalog.resolve(sample).expect("catalog metadata");

    assert_eq!(reading.value, NumericValue::F64(Finite::new(42.5).unwrap()));
    assert_eq!(reading.signal.metric.as_str(), "temperature");
    assert_eq!(reading.signal.instance.as_str(), "CPU0Temp");
    assert_eq!(reading.signal.unit.as_str(), "Cel");
    assert_eq!(reading.signal.revision, 0);
    assert_eq!(
        reading
            .signal
            .bounds
            .and_then(nv_telemetry_core::ValueRange::lower),
        Some(Finite::new(-10.0).unwrap())
    );
}

#[test]
fn refreshing_unchanged_metadata_keeps_the_shared_descriptor() {
    let sensor = fixture();
    let first = context();
    let later = context_at(OBSERVED_AT + 600);

    let mut catalog = SignalCatalog::new();
    let added = catalog
        .upsert(project_at::<SensorMetadataProjection>(&sensor, &first))
        .expect("catalog capacity");
    let installed = match added {
        SignalUpdate::Added(descriptor) => descriptor,
        other => panic!("expected a new signal, got {other:?}"),
    };

    let refreshed = catalog
        .upsert(project_at::<SensorMetadataProjection>(&sensor, &later))
        .expect("catalog capacity");

    assert!(matches!(refreshed, SignalUpdate::Unchanged(_)));
    assert!(Arc::ptr_eq(refreshed.descriptor(), &installed));
    assert_eq!(refreshed.descriptor().revision, 0);
    assert_eq!(refreshed.descriptor().observed_at, first.observed_at());
}

#[test]
fn confirmation_time_advances_on_refresh_and_drives_pruning() {
    let sensor = fixture();
    let first = context();
    let later = context_at(OBSERVED_AT + 600);
    let key = SignalKey::from("/redfish/v1/Chassis/1/Sensors/CPU0Temp");

    let mut catalog = SignalCatalog::new();
    for context in [&first, &later] {
        catalog
            .upsert(project_at::<SensorMetadataProjection>(&sensor, context))
            .expect("catalog capacity");
    }

    assert_eq!(
        catalog.get(&key).expect("descriptor").observed_at,
        first.observed_at()
    );
    assert_eq!(catalog.last_confirmed_at(&key), Some(later.observed_at()));

    catalog.retain_confirmed_since(later.observed_at());
    assert_eq!(catalog.len(), 1);

    let after_removal = Timestamp::new(OBSERVED_AT + 1_200, 0).expect("valid timestamp");
    catalog.retain_confirmed_since(after_removal);
    assert!(catalog.is_empty());
}

#[test]
fn changed_metadata_replaces_the_descriptor_at_the_next_revision() {
    let first = context();
    let later = context_at(OBSERVED_AT + 1);
    let mut sensor = fixture();
    let mut catalog = SignalCatalog::new();
    catalog
        .upsert(project_at::<SensorMetadataProjection>(&sensor, &first))
        .expect("catalog capacity");

    sensor.reading_units = Some(Some("K".to_owned()));
    let update = catalog
        .upsert(project_at::<SensorMetadataProjection>(&sensor, &later))
        .expect("catalog capacity");

    let SignalUpdate::Revised {
        descriptor,
        previous,
        ..
    } = update
    else {
        panic!("expected a revised signal");
    };
    assert_eq!(descriptor.unit.as_str(), "K");
    assert_eq!(descriptor.revision, 1);
    assert_eq!(previous.revision, 0);
}

#[test]
fn a_sample_from_another_acquisition_route_reuses_sensor_metadata() {
    let context = context_at(OBSERVED_AT + 10);
    let sensor = fixture();
    let record = project_at::<SensorMetadataProjection>(&sensor, &context);
    let mut catalog = SignalCatalog::new();
    catalog.upsert(record).expect("catalog capacity");

    let sample = SignalSample::new(
        "metric-report:ThermalReport",
        "/redfish/v1/Chassis/1/Sensors/CPU0Temp#/Reading",
        Finite::new(43.25).expect("finite reading"),
    )
    .with_observed_at(context.observed_at());
    let reading = catalog.resolve(sample).expect("same canonical signal");

    let key = SignalKey::from("/redfish/v1/Chassis/1/Sensors/CPU0Temp");
    let descriptor = catalog.get(&key).expect("catalog descriptor");
    assert!(Arc::ptr_eq(&reading.signal, descriptor));
    assert_eq!(
        reading.value,
        NumericValue::F64(Finite::new(43.25).unwrap())
    );
}
