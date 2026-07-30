// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! What the Sensor projections assert about a device, and how a consumer
//! joins the two halves back together.

use nv_redfish::schema::sensor::Sensor;
use nv_telemetry_core::Finite;
use nv_telemetry_core::NumericValue;
use nv_telemetry_core::PropertyValue;
use nv_telemetry_core::ReadingKind;
use nv_telemetry_core::ResourceGraph;
use nv_telemetry_core::Subject;
use nv_telemetry_core::Timestamp;
use nv_telemetry_redfish::Project;
use nv_telemetry_redfish::ProjectionIssue;
use nv_telemetry_redfish::ProjectionIssueKind;
use nv_telemetry_redfish::SensorMetadataProjection;
use nv_telemetry_redfish::SensorProjectionContext;
use nv_telemetry_redfish::SensorResourceProjection;
use nv_telemetry_redfish::SensorSampleProjection;
use nv_telemetry_redfish::SignalCatalog;
use nv_telemetry_redfish::SignalKey;
use serde_json::json;
use serde_json::Value;

fn context() -> SensorProjectionContext {
    SensorProjectionContext::new(Timestamp::new(1_721_000_000, 0).expect("valid timestamp"))
}

fn fixture_json() -> Value {
    serde_json::from_str(include_str!("fixtures/sensor.json")).expect("valid fixture JSON")
}

fn sensor_from(json: Value) -> Sensor {
    serde_json::from_value(json).expect("the compiled schema accepts the fixture")
}

fn fixture() -> Sensor {
    sensor_from(fixture_json())
}

fn project<P: Project<Sensor, SensorProjectionContext>>(sensor: &Sensor) -> P::Output {
    let result = P::project(sensor, &context());
    assert!(
        result.issues().is_empty(),
        "unexpected issues: {:?}",
        result.issues()
    );
    result.into_parts().0.expect("projection output")
}

#[test]
fn a_reading_finds_its_thresholds_through_the_subject_they_share() {
    let sensor = fixture();
    let record = project::<SensorMetadataProjection>(&sensor);
    let sample = project::<SensorSampleProjection>(&sensor);
    let resource = project::<SensorResourceProjection>(&sensor);

    let mut catalog = SignalCatalog::new();
    catalog.upsert(record);
    let reading = catalog.resolve(sample).expect("catalogued metadata");
    let graph = ResourceGraph::new(vec![resource], Vec::new()).expect("a valid graph");

    // The join the convergence engine makes: a reading names its subject, and
    // the graph answers what that subject is configured to.
    let observed = graph
        .get(&reading.signal.subject)
        .expect("the graph holds the sensor the reading names");

    assert_eq!(
        reading.signal.subject,
        Subject::new("sensor", "1/CPU0Temp"),
        "the chassis scopes an id that is unique only within its collection"
    );
    assert_eq!(
        observed.source_key.as_str(),
        "/redfish/v1/Chassis/1/Sensors/CPU0Temp",
        "the URI stays the source location rather than becoming the identity"
    );
    assert_eq!(
        observed.properties.get("upper_critical"),
        Some(&PropertyValue::F64(Finite::new(90.0).unwrap()))
    );
    assert_eq!(reading.value, NumericValue::F64(Finite::new(42.5).unwrap()));
}

#[test]
fn a_threshold_the_device_leaves_unset_is_stated_rather_than_omitted() {
    let mut json = fixture_json();
    json["Thresholds"] = json!({ "UpperCaution": { "Reading": 80.0 } });
    let resource = project::<SensorResourceProjection>(&sensor_from(json));

    assert_eq!(
        resource.properties.get("upper_caution"),
        Some(&PropertyValue::F64(Finite::new(80.0).unwrap()))
    );
    // The device implements thresholds and reports no upper critical, which
    // is a different claim from this projection not carrying one.
    assert_eq!(
        resource.properties.get("upper_critical"),
        Some(&PropertyValue::Null)
    );
}

#[test]
fn a_device_without_thresholds_makes_no_claim_about_them() {
    let mut json = fixture_json();
    json.as_object_mut()
        .expect("a JSON object")
        .remove("Thresholds");
    let resource = project::<SensorResourceProjection>(&sensor_from(json));

    assert_eq!(resource.properties.get("upper_caution"), None);
    assert_eq!(resource.properties.get("upper_critical"), None);
    assert_eq!(
        resource.properties.get("name"),
        Some(&PropertyValue::String("CPU 0 Temperature".into())),
        "state the device does report survives a device that reports no thresholds"
    );
}

#[test]
fn the_same_sensor_id_in_two_chassis_stays_two_subjects() {
    let mut second = fixture_json();
    second["@odata.id"] = json!("/redfish/v1/Chassis/2/Sensors/CPU0Temp");

    let first = project::<SensorResourceProjection>(&fixture());
    let second = project::<SensorResourceProjection>(&sensor_from(second));

    assert_ne!(first.subject, second.subject);
    // A graph rejects a repeated subject, so holding both proves they differ.
    ResourceGraph::new(vec![first, second], Vec::new())
        .expect("two chassis may report the same sensor id");
}

#[test]
fn a_sensor_reporting_no_units_still_produces_a_signal() {
    let mut json = fixture_json();
    json.as_object_mut()
        .expect("a JSON object")
        .remove("ReadingUnits");
    let sensor = sensor_from(json);

    let record = project::<SensorMetadataProjection>(&sensor);
    let sample = project::<SensorSampleProjection>(&sensor);

    assert_eq!(record.descriptor().unit.as_str(), "1");
    let mut catalog = SignalCatalog::new();
    catalog.upsert(record);
    catalog
        .resolve(sample)
        .expect("a unitless sensor is still collected");
}

#[test]
fn a_reading_that_accumulates_is_a_counter_rather_than_a_gauge() {
    let mut json = fixture_json();
    json["ReadingType"] = json!("EnergykWh");
    json["ReadingUnits"] = json!("kW.h");
    let energy = project::<SensorMetadataProjection>(&sensor_from(json));

    // Redfish defines energy as an integral since the last reset, so a
    // consumer differences it where it would average a temperature.
    assert_eq!(energy.descriptor().kind, ReadingKind::Counter);
    assert_eq!(
        project::<SensorMetadataProjection>(&fixture())
            .descriptor()
            .kind,
        ReadingKind::Gauge
    );
}

#[test]
fn an_empty_unit_reads_the_same_as_no_unit_at_all() {
    let mut json = fixture_json();
    json["ReadingUnits"] = json!("");
    let record = project::<SensorMetadataProjection>(&sensor_from(json));

    assert_eq!(record.descriptor().unit.as_str(), "1");
}

#[test]
fn a_reading_the_model_cannot_hold_is_invalid_rather_than_missing() {
    let mut sensor = fixture();
    sensor.reading = Some(Some(f64::NAN));

    let result = SensorSampleProjection::project(&sensor, &context());

    assert!(result.output().is_none());
    let issue = result
        .issues()
        .iter()
        .find(|issue| issue.path() == "Sensor.Reading")
        .expect("the reading is reported");
    assert!(
        matches!(issue.kind(), ProjectionIssueKind::Invalid { .. }),
        "a device that sent an unusable number is not a device that stayed quiet, got {:?}",
        issue.kind()
    );
}

#[test]
fn a_sensor_that_sent_no_reading_is_missing_rather_than_invalid() {
    let mut sensor = fixture();
    sensor.reading = None;

    let result = SensorSampleProjection::project(&sensor, &context());

    assert!(result.output().is_none());
    assert_eq!(
        result.issues().first().map(ProjectionIssue::kind),
        Some(&ProjectionIssueKind::MissingRequired)
    );
}

#[test]
fn a_metric_report_property_resolves_to_the_sensor_it_names() {
    let sensor = fixture();
    let mut catalog = SignalCatalog::new();
    catalog.upsert(project::<SensorMetadataProjection>(&sensor));

    // A metric report addresses the reading inside the resource.
    let reported = SignalKey::from("/redfish/v1/Chassis/1/Sensors/CPU0Temp#/Reading");

    assert!(
        catalog.get(&reported).is_some(),
        "a property fragment names the same signal as the sensor resource"
    );
}

#[test]
fn a_sensor_outside_a_chassis_cannot_be_given_a_subject() {
    let mut json = fixture_json();
    json["@odata.id"] = json!("/redfish/v1/Systems/1/Sensors/CPU0Temp");
    let sensor = sensor_from(json);

    let result = SensorMetadataProjection::project(&sensor, &context());

    assert!(result.output().is_none());
    let issue = result.issues().first().expect("the URI is reported");
    assert_eq!(issue.path(), "Sensor.@odata.id");
    assert!(
        matches!(issue.kind(), ProjectionIssueKind::Invalid { .. }),
        "an unrecognised location is reported rather than guessed at"
    );
}
