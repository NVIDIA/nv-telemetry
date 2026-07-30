// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use nv_redfish::schema::sensor::Sensor;
use nv_telemetry_core::Finite;
use nv_telemetry_core::NumericValue;
use nv_telemetry_core::Timestamp;
use nv_telemetry_redfish::FieldValue;
use nv_telemetry_redfish::Fields;
use nv_telemetry_redfish::Project;
use nv_telemetry_redfish::ProjectionResult;
use nv_telemetry_redfish::SensorMetadataProjection;
use nv_telemetry_redfish::SensorProjectionContext;
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

fn fixture() -> Sensor {
    serde_json::from_str(include_str!("fixtures/sensor.json")).expect("valid Redfish fixture")
}

fn context(seconds: i64) -> SensorProjectionContext {
    SensorProjectionContext::new(Timestamp::new(seconds, 0).expect("valid timestamp"))
}

#[test]
fn project_can_be_extended_outside_the_crate() {
    let result = ExternalProjection::project(&ExternalSource { value: Some(21) }, &());

    assert_eq!(result.output(), Some(&42));
    assert!(result.issues().is_empty());
}

#[test]
fn compiled_sensor_metadata_and_sample_join_without_an_adapter() {
    let sensor = fixture();
    let context = context(1_721_000_000);
    let record = SensorMetadataProjection::project(&sensor, &context)
        .into_parts()
        .0
        .expect("metadata output");
    let sample = SensorSampleProjection::project(&sensor, &context)
        .into_parts()
        .0
        .expect("sample output");
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
    let first = context(1_721_000_000);
    let later = context(1_721_000_600);

    let mut catalog = SignalCatalog::new();
    let added = catalog
        .upsert(
            SensorMetadataProjection::project(&sensor, &first)
                .into_parts()
                .0
                .expect("metadata output"),
        )
        .expect("catalog capacity");
    let installed = match added {
        SignalUpdate::Added(descriptor) => descriptor,
        other => panic!("expected a new signal, got {other:?}"),
    };

    let refreshed = catalog
        .upsert(
            SensorMetadataProjection::project(&sensor, &later)
                .into_parts()
                .0
                .expect("metadata output"),
        )
        .expect("catalog capacity");

    assert!(matches!(refreshed, SignalUpdate::Unchanged(_)));
    assert!(Arc::ptr_eq(refreshed.descriptor(), &installed));
    assert_eq!(refreshed.descriptor().revision, 0);
    assert_eq!(refreshed.descriptor().observed_at, first.observed_at());
}

#[test]
fn confirmation_time_advances_on_refresh_and_drives_pruning() {
    let sensor = fixture();
    let first = context(1_721_000_000);
    let later = context(1_721_000_600);
    let key = SignalKey::from("/redfish/v1/Chassis/1/Sensors/CPU0Temp");

    let mut catalog = SignalCatalog::new();
    for context in [&first, &later] {
        catalog
            .upsert(
                SensorMetadataProjection::project(&sensor, context)
                    .into_parts()
                    .0
                    .expect("metadata output"),
            )
            .expect("catalog capacity");
    }

    assert_eq!(
        catalog.get(&key).expect("descriptor").observed_at,
        first.observed_at()
    );
    assert_eq!(catalog.last_confirmed_at(&key), Some(later.observed_at()));

    catalog.retain_confirmed_since(later.observed_at());
    assert_eq!(catalog.len(), 1);

    let after_removal = Timestamp::new(1_721_001_200, 0).expect("valid timestamp");
    catalog.retain_confirmed_since(after_removal);
    assert!(catalog.is_empty());
}

#[test]
fn changed_metadata_replaces_the_descriptor_at_the_next_revision() {
    let first = context(1_721_000_000);
    let later = context(1_721_000_001);
    let mut sensor = fixture();
    let mut catalog = SignalCatalog::new();
    catalog
        .upsert(
            SensorMetadataProjection::project(&sensor, &first)
                .into_parts()
                .0
                .expect("metadata output"),
        )
        .expect("catalog capacity");

    sensor.reading_units = Some(Some("K".to_owned()));
    let update = catalog
        .upsert(
            SensorMetadataProjection::project(&sensor, &later)
                .into_parts()
                .0
                .expect("metadata output"),
        )
        .expect("catalog capacity");

    let SignalUpdate::Revised {
        descriptor,
        previous,
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
    let context = context(1_721_000_010);
    let sensor = fixture();
    let record = SensorMetadataProjection::project(&sensor, &context)
        .into_parts()
        .0
        .expect("metadata output");
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
