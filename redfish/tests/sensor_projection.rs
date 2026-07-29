// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

// The fixtures mirror nv-redfish's shape, where the outer Option separates an
// absent field from a JSON null carried by the inner one.
#![allow(clippy::option_option)]

use std::sync::Arc;

use nv_telemetry_core::finite;
use nv_telemetry_core::Finite;
use nv_telemetry_core::Name;
use nv_telemetry_core::NumericValue;
use nv_telemetry_core::ReadingKind;
use nv_telemetry_core::ReportedState;
use nv_telemetry_core::SignalDescriptor;
use nv_telemetry_core::Subject;
use nv_telemetry_core::Timestamp;
use nv_telemetry_redfish::telemetry_projection;
use nv_telemetry_redfish::FieldValue;
use nv_telemetry_redfish::Project;
use nv_telemetry_redfish::ProjectionIssueKind;
use nv_telemetry_redfish::SignalCatalog;
use nv_telemetry_redfish::SignalDescriptorRecord;
use nv_telemetry_redfish::SignalKey;
use nv_telemetry_redfish::SignalSample;
use nv_telemetry_redfish::SignalUpdate;

#[derive(Clone, Debug, serde::Deserialize)]
struct SensorSchema {
    #[serde(rename = "@odata.id")]
    odata_id: Option<String>,
    #[serde(rename = "Id")]
    id: Option<String>,
    #[serde(rename = "Reading")]
    reading: Option<Option<f64>>,
    #[serde(rename = "ReadingType")]
    reading_type: Option<Option<String>>,
    #[serde(rename = "ReadingUnits")]
    reading_units: Option<Option<String>>,
    #[serde(rename = "Status")]
    status: Option<StatusSchema>,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct StatusSchema {
    #[serde(rename = "Health")]
    health: Option<Option<String>>,
}

#[derive(Clone, Debug)]
struct MetricValueSchema {
    report_id: String,
    metric_property: Option<String>,
    metric_value: Option<String>,
}

#[derive(Clone, Copy, Debug)]
struct ProjectionContext {
    observed_at: Timestamp,
}

telemetry_projection! {
    FixtureSensorMetadataProjection(SensorSchema, ProjectionContext) -> SignalDescriptorRecord
    |sensor, context| {
        required {
            source_key: "Sensor.@odata.id" =>
                FieldValue::from_option(sensor.odata_id.clone()),
            instance: "Sensor.Id" =>
                FieldValue::from_option(sensor.id.clone()),
            metric: "Sensor.ReadingType" =>
                FieldValue::from_option(
                    sensor
                        .reading_type
                        .clone()
                        .flatten()
                        .map(|value| value.to_ascii_lowercase())
                ),
            unit: "Sensor.ReadingUnits" =>
                FieldValue::from_nested_option(sensor.reading_units.clone())
        }
        optional { }
        build {
            let descriptor = SignalDescriptor::new(
                Subject::new("sensor", source_key.clone()),
                metric,
                instance,
                ReadingKind::Gauge,
                unit,
                context.observed_at,
            );
            SignalDescriptorRecord::new(source_key, descriptor)
        }
    }
}

telemetry_projection! {
    FixtureSensorSampleProjection(SensorSchema, ProjectionContext) -> SignalSample
    |sensor, context| {
        required {
            source_key: "Sensor.@odata.id" =>
                FieldValue::from_option(sensor.odata_id.clone()),
            signal_key: "Sensor.@odata.id" =>
                FieldValue::from_option(sensor.odata_id.clone()),
            value: "Sensor.Reading" =>
                FieldValue::from_option(finite_reading(sensor.reading.flatten()))
        }
        optional {
            reported_state = sensor.status
                .as_ref()
                .and_then(|status| status.health.clone().flatten())
                .map(|health| ReportedState::new(None, Some(health.into())))
        }
        build {
            let mut sample = SignalSample::new(source_key, signal_key, value)
                .with_observed_at(context.observed_at);
            if let Some(reported_state) = reported_state {
                sample = sample.with_reported_state(reported_state);
            }
            sample
        }
    }
}

telemetry_projection! {
    MetricReportSampleProjection(MetricValueSchema, ProjectionContext) -> SignalSample
    |metric_value, context| {
        required {
            signal_key: "MetricValue.MetricProperty" =>
                FieldValue::from_option(metric_value.metric_property.clone()),
            value: "MetricValue.MetricValue" =>
                parse_metric_value(metric_value.metric_value.as_deref())
        }
        optional {
            source_key = format!("metric-report:{}", metric_value.report_id)
        }
        build {
            SignalSample::new(source_key, signal_key, value)
                .with_observed_at(context.observed_at)
        }
    }
}

fn finite_reading(value: Option<f64>) -> Option<Finite> {
    value.and_then(|value| Finite::new(value).ok())
}

fn parse_metric_value(value: Option<&str>) -> FieldValue<Finite> {
    let Some(value) = value else {
        return FieldValue::missing();
    };
    match value.parse::<f64>() {
        Ok(value) => FieldValue::from_result(Finite::new(value)),
        Err(error) => FieldValue::invalid(error.to_string()),
    }
}

fn fixture() -> SensorSchema {
    serde_json::from_str(include_str!("fixtures/sensor.json")).expect("valid Redfish fixture")
}

#[test]
fn sensor_metadata_and_samples_are_projected_in_two_layers() {
    let context = ProjectionContext {
        observed_at: Timestamp::new(1_721_000_000, 0).expect("valid timestamp"),
    };
    let sensor = fixture();

    let metadata = FixtureSensorMetadataProjection::project(&sensor, &context);
    assert!(metadata.issues().is_empty());
    let mut catalog = SignalCatalog::new();
    let record = metadata.into_parts().0.expect("metadata output");
    catalog.upsert(record);

    let sample = FixtureSensorSampleProjection::project(&sensor, &context)
        .into_parts()
        .0
        .expect("sample output");
    let reading = catalog.resolve(sample).expect("catalog metadata");

    assert_eq!(reading.value, NumericValue::F64(finite!(42.5)));
    assert_eq!(reading.signal.unit.as_str(), "Cel");
    assert_eq!(reading.signal.metric.as_str(), "temperature");
    assert_eq!(
        reading
            .reported_state
            .as_ref()
            .and_then(|state| state.health.as_ref())
            .map(Name::as_str),
        Some("OK")
    );
}

#[test]
fn compiled_nv_redfish_sensor_projects_without_an_adapter() {
    let sensor: nv_redfish::schema::sensor::Sensor =
        serde_json::from_str(include_str!("fixtures/sensor.json"))
            .expect("fixture matches compiled nv-redfish Sensor");
    let observed_at = Timestamp::new(1_721_000_000, 0).expect("valid timestamp");
    let context = nv_telemetry_redfish::SensorProjectionContext::new(observed_at);

    let record = nv_telemetry_redfish::SensorMetadataProjection::project(&sensor, &context)
        .into_parts()
        .0
        .expect("metadata output");
    let sample = nv_telemetry_redfish::SensorSampleProjection::project(&sensor, &context)
        .into_parts()
        .0
        .expect("sample output");
    let mut catalog = SignalCatalog::new();
    catalog.upsert(record);
    let reading = catalog.resolve(sample).expect("catalog metadata");

    assert_eq!(reading.signal.metric.as_str(), "temperature");
    assert_eq!(reading.signal.instance.as_str(), "CPU0Temp");
    assert_eq!(reading.signal.unit.as_str(), "Cel");
    assert_eq!(reading.signal.revision, 0);
    assert_eq!(
        reading.signal.bounds.and_then(|bounds| bounds.lower),
        Some(finite!(-10.0))
    );
}

#[test]
fn refreshing_unchanged_metadata_keeps_the_shared_descriptor() {
    let sensor = fixture();
    let first = ProjectionContext {
        observed_at: Timestamp::new(1_721_000_000, 0).expect("valid timestamp"),
    };
    let later = ProjectionContext {
        observed_at: Timestamp::new(1_721_000_600, 0).expect("valid timestamp"),
    };

    let mut catalog = SignalCatalog::new();
    let added = catalog.upsert(
        FixtureSensorMetadataProjection::project(&sensor, &first)
            .into_parts()
            .0
            .expect("metadata output"),
    );
    let installed = match added {
        SignalUpdate::Added(descriptor) => descriptor,
        other => panic!("expected a new signal, got {other:?}"),
    };

    let refreshed = catalog.upsert(
        FixtureSensorMetadataProjection::project(&sensor, &later)
            .into_parts()
            .0
            .expect("metadata output"),
    );

    assert!(matches!(refreshed, SignalUpdate::Unchanged(_)));
    assert!(Arc::ptr_eq(refreshed.descriptor(), &installed));
    assert_eq!(refreshed.descriptor().revision, 0);
    assert_eq!(refreshed.descriptor().observed_at, first.observed_at);
}

#[test]
fn confirmation_time_advances_on_refresh_and_drives_pruning() {
    let sensor = fixture();
    let first = ProjectionContext {
        observed_at: Timestamp::new(1_721_000_000, 0).expect("valid timestamp"),
    };
    let later = ProjectionContext {
        observed_at: Timestamp::new(1_721_000_600, 0).expect("valid timestamp"),
    };
    let key = SignalKey::from("/redfish/v1/Chassis/1/Sensors/CPU0Temp");

    let mut catalog = SignalCatalog::new();
    for context in [&first, &later] {
        catalog.upsert(
            FixtureSensorMetadataProjection::project(&sensor, context)
                .into_parts()
                .0
                .expect("metadata output"),
        );
    }

    assert_eq!(
        catalog.get(&key).expect("descriptor").observed_at,
        first.observed_at
    );
    assert_eq!(catalog.last_confirmed_at(&key), Some(later.observed_at));

    catalog.retain_confirmed_since(later.observed_at);
    assert_eq!(catalog.len(), 1);

    let after_removal = Timestamp::new(1_721_001_200, 0).expect("valid timestamp");
    catalog.retain_confirmed_since(after_removal);
    assert!(catalog.is_empty());
}

#[test]
fn changed_metadata_replaces_the_descriptor_at_the_next_revision() {
    let context = ProjectionContext {
        observed_at: Timestamp::new(1_721_000_000, 0).expect("valid timestamp"),
    };
    let mut sensor = fixture();
    let mut catalog = SignalCatalog::new();
    catalog.upsert(
        FixtureSensorMetadataProjection::project(&sensor, &context)
            .into_parts()
            .0
            .expect("metadata output"),
    );

    sensor.reading_units = Some(Some("K".to_owned()));
    let update = catalog.upsert(
        FixtureSensorMetadataProjection::project(&sensor, &context)
            .into_parts()
            .0
            .expect("metadata output"),
    );

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
fn metric_report_sample_reuses_sensor_metadata() {
    let context = ProjectionContext {
        observed_at: Timestamp::new(1_721_000_010, 0).expect("valid timestamp"),
    };
    let sensor = fixture();
    let record = FixtureSensorMetadataProjection::project(&sensor, &context)
        .into_parts()
        .0
        .expect("metadata output");
    let mut catalog = SignalCatalog::new();
    catalog.upsert(record);

    let metric_value = MetricValueSchema {
        report_id: "ThermalReport".to_owned(),
        metric_property: sensor.odata_id,
        metric_value: Some("43.25".to_owned()),
    };
    let sample = MetricReportSampleProjection::project(&metric_value, &context)
        .into_parts()
        .0
        .expect("metric sample output");
    let reading = catalog.resolve(sample).expect("same canonical signal");

    let key = SignalKey::from("/redfish/v1/Chassis/1/Sensors/CPU0Temp");
    let descriptor = catalog.get(&key).expect("catalog descriptor");
    assert!(Arc::ptr_eq(&reading.signal, descriptor));
    assert_eq!(reading.value, NumericValue::F64(finite!(43.25)));
}

#[test]
fn missing_sensor_metadata_reports_every_required_field() {
    let empty = SensorSchema {
        odata_id: None,
        id: None,
        reading: None,
        reading_type: None,
        reading_units: None,
        status: None,
    };
    let context = ProjectionContext {
        observed_at: Timestamp::new(1, 0).expect("valid timestamp"),
    };

    let result = FixtureSensorMetadataProjection::project(&empty, &context);

    assert!(result.output().is_none());
    assert_eq!(result.issues().len(), 4);
    assert!(result
        .issues()
        .iter()
        .all(|issue| issue.kind() == &ProjectionIssueKind::MissingRequired));
}
