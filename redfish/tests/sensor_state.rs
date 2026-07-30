// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! What the Sensor projections assert about a device, and how a consumer
//! joins the two halves back together.

use nv_redfish::schema::sensor::Sensor;
use nv_telemetry_core::DurationValue;
use nv_telemetry_core::Finite;
use nv_telemetry_core::NumericValue;
use nv_telemetry_core::ObservedResource;
use nv_telemetry_core::PropertyMap;
use nv_telemetry_core::PropertyValue;
use nv_telemetry_core::ReadingKind;
use nv_telemetry_core::ResourceGraph;
use nv_telemetry_core::SourceKey;
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
    catalog.upsert(record).expect("catalog capacity");
    let reading = catalog.resolve(sample).expect("catalogued metadata");
    let (resource, relation) = resource.into_parts();
    let graph = ResourceGraph::new(vec![resource], Vec::new()).expect("a valid graph");

    // The join the convergence engine makes: a reading names its subject, and
    // the graph answers what that subject is configured to.
    let observed = graph
        .get(&reading.signal.subject)
        .expect("the graph holds the sensor the reading names");

    assert_eq!(
        reading.signal.subject,
        Subject::new("sensor".into(), "chassis/1/CPU0Temp".into()),
        "the chassis scopes an id that is unique only within its collection"
    );
    assert_eq!(relation.source, Subject::new("chassis".into(), "1".into()));
    assert_eq!(relation.target, reading.signal.subject);
    assert_eq!(relation.kind.as_str(), "contains");
    assert_eq!(
        observed.source_key.as_str(),
        "/redfish/v1/Chassis/1/Sensors/CPU0Temp",
        "the URI stays the source location rather than becoming the identity"
    );
    assert_eq!(
        observed
            .properties
            .get("upper_critical")
            .and_then(|value| match value {
                PropertyValue::Object(properties) => properties.get("reading"),
                _ => None,
            }),
        Some(&PropertyValue::F64(Finite::new(90.0).unwrap()))
    );
    assert_eq!(reading.value, NumericValue::F64(Finite::new(42.5).unwrap()));
}

#[test]
fn a_complete_parent_and_projected_sensor_assemble_into_one_graph() {
    let sensor_record = project::<SensorResourceProjection>(&fixture());
    let (sensor, relation) = sensor_record.into_parts();
    let parent_subject = relation.source.clone();
    let parent = ObservedResource::complete(
        parent_subject.clone(),
        SourceKey::from("/redfish/v1/Chassis/1"),
        PropertyMap::empty(),
    );

    let graph = ResourceGraph::new(vec![parent, sensor.clone()], vec![relation.clone()])
        .expect("the emitted relation joins a complete parent to its sensor");

    assert_eq!(graph.resources().len(), 2);
    assert_eq!(graph.relations(), &[relation]);
    assert_eq!(
        graph.get(&parent_subject).map(|resource| &resource.subject),
        Some(&parent_subject)
    );
    assert_eq!(
        graph.get(&sensor.subject).map(|resource| &resource.subject),
        Some(&sensor.subject)
    );
}

#[test]
fn a_threshold_the_device_leaves_unset_is_stated_rather_than_omitted() {
    let mut json = fixture_json();
    json["Thresholds"] = json!({ "UpperCaution": { "Reading": 80.0 } });
    let record = project::<SensorResourceProjection>(&sensor_from(json));
    let resource = record.resource();

    assert_eq!(
        resource
            .properties
            .get("upper_caution")
            .and_then(|value| match value {
                PropertyValue::Object(properties) => properties.get("reading"),
                _ => None,
            }),
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
    let record = project::<SensorResourceProjection>(&sensor_from(json));
    let resource = record.resource();

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

    assert_ne!(first.resource().subject, second.resource().subject);
    // A graph rejects a repeated subject, so holding both proves they differ.
    ResourceGraph::new(
        vec![first.into_parts().0, second.into_parts().0],
        Vec::new(),
    )
    .expect("two chassis may report the same sensor id");
}

#[test]
fn every_power_distribution_parent_collection_scopes_its_sensors() {
    let cases = [
        ("FloorPDUs", "floor_pdus"),
        ("RackPDUs", "rack_pdus"),
        ("Switchgear", "switchgear"),
        ("TransferSwitches", "transfer_switches"),
        ("PowerShelves", "power_shelves"),
        ("ElectricalBuses", "electrical_buses"),
    ];

    for (collection, subject_collection) in cases {
        let mut json = fixture_json();
        json["@odata.id"] = json!(format!(
            "/redfish/v1/PowerEquipment/{collection}/PDU1/Sensors/CPU0Temp"
        ));
        let sensor = sensor_from(json);
        let metadata = project::<SensorMetadataProjection>(&sensor);
        let resource = project::<SensorResourceProjection>(&sensor);

        assert_eq!(
            metadata.descriptor().subject,
            Subject::new(
                "sensor".into(),
                format!("power_distribution/{subject_collection}/PDU1/CPU0Temp").into(),
            ),
            "{collection}"
        );
        assert_eq!(
            resource.parent_relation().source,
            Subject::new(
                "power_distribution".into(),
                format!("{subject_collection}/PDU1").into(),
            ),
            "{collection}"
        );
        assert_eq!(
            resource.parent_relation().target,
            resource.resource().subject,
            "{collection}"
        );
    }
}

#[test]
fn reading_time_and_etag_are_preserved_in_their_respective_outputs() {
    let mut json = fixture_json();
    json["ReadingTime"] = json!("1970-01-01T00:00:01Z");
    json["@odata.etag"] = json!("W/\"sensor-version\"");
    let sensor = sensor_from(json);

    let record = project::<SensorMetadataProjection>(&sensor);
    let sample = project::<SensorSampleProjection>(&sensor);
    let resource = project::<SensorResourceProjection>(&sensor);
    let mut catalog = SignalCatalog::new();
    catalog.upsert(record).expect("catalog capacity");
    let reading = catalog.resolve(sample).expect("catalogued metadata");

    assert_eq!(
        reading.observed_at,
        Some(Timestamp::new(1, 0).expect("valid timestamp"))
    );
    assert_eq!(
        resource
            .resource()
            .version
            .as_ref()
            .map(nv_telemetry_core::Name::as_str),
        Some("W/\"sensor-version\"")
    );
}

#[test]
fn a_pre_1601_reading_time_is_preserved() {
    let mut json = fixture_json();
    json["ReadingTime"] = json!("1500-01-01T00:00:00.123456789Z");
    let sensor = sensor_from(json);

    let record = project::<SensorMetadataProjection>(&sensor);
    let sample = project::<SensorSampleProjection>(&sensor);
    let mut catalog = SignalCatalog::new();
    catalog.upsert(record).expect("catalog capacity");
    let reading = catalog.resolve(sample).expect("catalogued metadata");

    assert_eq!(
        reading.observed_at,
        Some(Timestamp::new(-14_831_769_600, 123_456_789).expect("valid timestamp"))
    );
}

#[test]
fn malformed_reading_time_is_rejected_at_the_typed_input_boundary() {
    let mut json = fixture_json();
    json["ReadingTime"] = json!("not-an-rfc3339-timestamp");

    serde_json::from_value::<Sensor>(json).expect_err("ReadingTime must be RFC 3339");
}

#[test]
fn all_threshold_slots_preserve_nested_behavior_metadata() {
    let mut json = fixture_json();
    let slots = [
        ("UpperCaution", "upper_caution"),
        ("UpperCritical", "upper_critical"),
        ("UpperFatal", "upper_fatal"),
        ("LowerCaution", "lower_caution"),
        ("LowerCritical", "lower_critical"),
        ("LowerFatal", "lower_fatal"),
        ("UpperCautionUser", "upper_caution_user"),
        ("UpperCriticalUser", "upper_critical_user"),
        ("LowerCautionUser", "lower_caution_user"),
        ("LowerCriticalUser", "lower_critical_user"),
    ];
    let thresholds = json
        .get_mut("Thresholds")
        .and_then(Value::as_object_mut)
        .expect("threshold object");
    for (source_name, _) in slots {
        thresholds.insert(
            source_name.to_owned(),
            json!({
                "Reading": 80.0,
                "Activation": "Increasing",
                "DwellTime": "PT2.5S",
                "HysteresisReading": 3.0,
                "HysteresisDuration": "PT1S"
            }),
        );
    }

    let record = project::<SensorResourceProjection>(&sensor_from(json));
    for (_, property_name) in slots {
        let value = record
            .resource()
            .properties
            .get(property_name)
            .expect("all ten slots are projected");
        let PropertyValue::Object(properties) = value else {
            panic!("a present threshold is a nested object");
        };
        assert_eq!(
            properties.get("reading"),
            Some(&PropertyValue::F64(Finite::new(80.0).unwrap()))
        );
        assert_eq!(
            properties.get("activation"),
            Some(&PropertyValue::String("increasing".into()))
        );
        assert_eq!(
            properties.get("dwell_time"),
            Some(&PropertyValue::Duration(
                DurationValue::new(2, 500_000_000).expect("valid duration")
            ))
        );
        assert_eq!(
            properties.get("hysteresis_reading"),
            Some(&PropertyValue::F64(Finite::new(3.0).unwrap()))
        );
        assert_eq!(
            properties.get("hysteresis_duration"),
            Some(&PropertyValue::Duration(
                DurationValue::new(1, 0).expect("valid duration")
            ))
        );
    }
}

#[test]
fn invalid_threshold_durations_are_reported_without_losing_the_resource() {
    let mut json = fixture_json();
    json["Thresholds"] = json!({
        "UpperCaution": {
            "Reading": 80.0,
            "DwellTime": "-PT1S",
            "HysteresisDuration": "-PT2S"
        }
    });

    let result = SensorResourceProjection::project(&sensor_from(json), &context());

    assert!(result.output().is_some());
    assert_eq!(result.issues().len(), 2);
    assert!(result.issues().iter().any(|issue| issue.path()
        == "Sensor.Thresholds.UpperCaution.DwellTime"
        && matches!(issue.kind(), ProjectionIssueKind::Invalid { .. })));
    assert!(result.issues().iter().any(|issue| issue.path()
        == "Sensor.Thresholds.UpperCaution.HysteresisDuration"
        && matches!(issue.kind(), ProjectionIssueKind::Invalid { .. })));
}

#[test]
fn threshold_numbers_preserve_missing_and_non_finite_semantics() {
    let mut sensor = fixture();
    let thresholds = sensor.thresholds.as_mut().expect("fixture thresholds");
    thresholds
        .upper_caution
        .as_mut()
        .expect("fixture upper caution")
        .reading = Some(Some(f64::NAN));
    thresholds
        .upper_critical
        .as_mut()
        .expect("fixture upper critical")
        .hysteresis_reading = Some(Some(f64::INFINITY));

    let result = SensorResourceProjection::project(&sensor, &context());
    let resource = result
        .output()
        .expect("invalid leaves do not block resource");
    let PropertyValue::Object(upper_caution) = resource
        .resource()
        .properties
        .get("upper_caution")
        .expect("configured threshold")
    else {
        panic!("threshold is a nested object");
    };

    assert_eq!(
        upper_caution.get("hysteresis_reading"),
        Some(&PropertyValue::Null),
        "a missing numeric leaf remains explicitly null"
    );
    assert_eq!(upper_caution.get("reading"), None);
    let PropertyValue::Object(upper_critical) = resource
        .resource()
        .properties
        .get("upper_critical")
        .expect("configured threshold")
    else {
        panic!("threshold is a nested object");
    };
    assert_eq!(upper_critical.get("hysteresis_reading"), None);
    assert_eq!(result.issues().len(), 2);
    assert!(result.issues().iter().any(|issue| issue.path()
        == "Sensor.Thresholds.UpperCaution.Reading"
        && matches!(issue.kind(), ProjectionIssueKind::Invalid { .. })));
    assert!(result.issues().iter().any(|issue| issue.path()
        == "Sensor.Thresholds.UpperCritical.HysteresisReading"
        && matches!(issue.kind(), ProjectionIssueKind::Invalid { .. })));
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
    catalog.upsert(record).expect("catalog capacity");
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
fn unsupported_reading_type_blocks_metadata_with_an_invalid_issue() {
    let mut json = fixture_json();
    json["ReadingType"] = json!("OEMReadingType");

    let result = SensorMetadataProjection::project(&sensor_from(json), &context());

    assert!(result.output().is_none());
    assert_eq!(result.issues().len(), 1);
    assert_eq!(result.issues()[0].path(), "Sensor.ReadingType");
    assert!(matches!(
        result.issues()[0].kind(),
        ProjectionIssueKind::Invalid { .. }
    ));
}

#[test]
fn unsupported_health_and_state_are_optional_invalid_issues() {
    let mut json = fixture_json();
    json["Status"] = json!({
        "State": "OEMState",
        "Health": "OEMHealth"
    });
    let sensor = sensor_from(json);
    let result = SensorSampleProjection::project(&sensor, &context());

    assert!(result.output().is_some());
    assert_eq!(result.issues().len(), 2);
    assert!(result.issues().iter().any(|issue| {
        issue.path() == "Sensor.Status.State"
            && matches!(issue.kind(), ProjectionIssueKind::Invalid { .. })
    }));
    assert!(result.issues().iter().any(|issue| {
        issue.path() == "Sensor.Status.Health"
            && matches!(issue.kind(), ProjectionIssueKind::Invalid { .. })
    }));

    let mut catalog = SignalCatalog::new();
    catalog
        .upsert(project::<SensorMetadataProjection>(&sensor))
        .expect("catalog capacity");
    let reading = catalog
        .resolve(result.into_parts().0.expect("sample output"))
        .expect("catalogued metadata");
    assert_eq!(reading.reported_state, None);
}

#[test]
fn non_finite_optional_ranges_are_issues_but_do_not_block_metadata() {
    let mut sensor = fixture();
    sensor.reading_range_min = Some(Some(f64::NAN));
    sensor.reading_range_max = Some(Some(f64::INFINITY));

    let result = SensorMetadataProjection::project(&sensor, &context());
    let descriptor = result
        .output()
        .expect("optional bounds do not block metadata");

    assert_eq!(descriptor.descriptor().bounds, None);
    assert_eq!(result.issues().len(), 2);
    assert!(result.issues().iter().any(|issue| {
        issue.path() == "Sensor.ReadingRangeMin"
            && matches!(issue.kind(), ProjectionIssueKind::Invalid { .. })
    }));
    assert!(result.issues().iter().any(|issue| {
        issue.path() == "Sensor.ReadingRangeMax"
            && matches!(issue.kind(), ProjectionIssueKind::Invalid { .. })
    }));
}

#[test]
fn inverted_optional_range_is_reported_and_omitted() {
    let mut json = fixture_json();
    json["ReadingRangeMin"] = json!(120.0);
    json["ReadingRangeMax"] = json!(110.0);

    let result = SensorMetadataProjection::project(&sensor_from(json), &context());
    let descriptor = result
        .output()
        .expect("optional bounds do not block metadata");

    assert_eq!(descriptor.descriptor().bounds, None);
    assert_eq!(result.issues().len(), 1);
    assert_eq!(result.issues()[0].path(), "Sensor.ReadingRangeMin");
    assert!(matches!(
        result.issues()[0].kind(),
        ProjectionIssueKind::Invalid { .. }
    ));
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
    catalog
        .upsert(project::<SensorMetadataProjection>(&sensor))
        .expect("catalog capacity");

    // A metric report addresses the reading inside the resource.
    let reported = SignalKey::from("/redfish/v1/Chassis/1/Sensors/CPU0Temp#/Reading");

    assert!(
        catalog.get(&reported).is_some(),
        "a property fragment names the same signal as the sensor resource"
    );
}

#[test]
fn a_sensor_outside_supported_parent_collections_has_no_subject() {
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
