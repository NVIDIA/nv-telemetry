// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! What the Sensor projections assert about a device, and how a consumer
//! joins the two halves back together.

mod common;

use common::context;
use common::fixture;
use common::fixture_json;
use common::project;
use common::sensor_from;
use common::threshold_leaf;
use nv_redfish::schema::sensor::Sensor;
use nv_telemetry_core::DurationValue;
use nv_telemetry_core::Finite;
use nv_telemetry_core::Health;
use nv_telemetry_core::Name;
use nv_telemetry_core::NumericValue;
use nv_telemetry_core::ObservedResource;
use nv_telemetry_core::OperatingState;
use nv_telemetry_core::PropertyMap;
use nv_telemetry_core::PropertyValue;
use nv_telemetry_core::ReadingKind;
use nv_telemetry_core::ResourceCompleteness;
use nv_telemetry_core::ResourceGraph;
use nv_telemetry_core::SourceKey;
use nv_telemetry_core::Subject;
use nv_telemetry_core::Timestamp;
use nv_telemetry_redfish::Project;
use nv_telemetry_redfish::ProjectionIssueKind;
use nv_telemetry_redfish::SensorMetadataProjection;
use nv_telemetry_redfish::SensorResourceProjection;
use nv_telemetry_redfish::SensorSampleProjection;
use nv_telemetry_redfish::SignalCatalog;
use nv_telemetry_redfish::SignalKey;
use nv_telemetry_redfish::SignalSample;
use nv_telemetry_redfish::SignalUpdate;
use serde_json::json;
use serde_json::Value;

fn finite(value: f64) -> Finite {
    Finite::new(value).expect("finite fixture value")
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
        threshold_leaf(observed, "upper_critical", "reading"),
        Some(&PropertyValue::F64(finite(90.0)))
    );
    assert_eq!(reading.value, NumericValue::F64(finite(42.5)));
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
        threshold_leaf(resource, "upper_caution", "reading"),
        Some(&PropertyValue::F64(finite(80.0)))
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
        resource.resource().version.as_ref().map(Name::as_str),
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
    let resource = record.resource();
    for (_, slot) in slots {
        assert_eq!(
            threshold_leaf(resource, slot, "reading"),
            Some(&PropertyValue::F64(finite(80.0))),
            "{slot}"
        );
        assert_eq!(
            threshold_leaf(resource, slot, "activation"),
            Some(&PropertyValue::String("increasing".into())),
            "{slot}"
        );
        assert_eq!(
            threshold_leaf(resource, slot, "dwell_time"),
            Some(&PropertyValue::Duration(
                DurationValue::new(2, 500_000_000).expect("valid duration")
            )),
            "{slot}"
        );
        assert_eq!(
            threshold_leaf(resource, slot, "hysteresis_reading"),
            Some(&PropertyValue::F64(finite(3.0))),
            "{slot}"
        );
        assert_eq!(
            threshold_leaf(resource, slot, "hysteresis_duration"),
            Some(&PropertyValue::Duration(
                DurationValue::new(1, 0).expect("valid duration")
            )),
            "{slot}"
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
fn a_missing_threshold_leaf_is_null_while_an_unusable_one_is_omitted() {
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
        .expect("invalid leaves do not block resource")
        .resource();

    assert_eq!(
        threshold_leaf(resource, "upper_caution", "hysteresis_reading"),
        Some(&PropertyValue::Null),
        "a leaf the device left unset configures nothing, which is a claim"
    );
    // A leaf the device sent unusably is left out instead. The resource is
    // partial, so an absent property claims nothing, which is what a rejected
    // value leaves behind; the issues below are what carry the failure.
    assert_eq!(threshold_leaf(resource, "upper_caution", "reading"), None);
    assert_eq!(
        threshold_leaf(resource, "upper_critical", "hysteresis_reading"),
        None
    );
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
    assert_eq!(
        result.issues()[0].path(),
        "Sensor.ReadingRangeMin,Sensor.ReadingRangeMax",
        "a contradiction between two properties blames neither of them alone"
    );
    assert!(matches!(
        result.issues()[0].kind(),
        ProjectionIssueKind::Invalid { .. }
    ));
}

#[test]
fn one_reported_range_edge_bounds_only_that_edge() {
    let cases = [
        ("ReadingRangeMax", Some(-10.0), None),
        ("ReadingRangeMin", None, Some(110.0)),
    ];

    for (dropped, lower, upper) in cases {
        let mut json = fixture_json();
        json.as_object_mut().expect("a JSON object").remove(dropped);
        let record = project::<SensorMetadataProjection>(&sensor_from(json));
        let bounds = record
            .descriptor()
            .bounds
            .expect("one reported edge is still a range");

        assert_eq!(bounds.lower(), lower.map(finite), "{dropped}");
        assert_eq!(bounds.upper(), upper.map(finite), "{dropped}");
    }
}

#[test]
fn a_sensor_that_sent_no_reading_is_missing_rather_than_invalid() {
    let mut sensor = fixture();
    sensor.reading = None;

    let result = SensorSampleProjection::project(&sensor, &context());

    assert!(result.output().is_none());
    assert_eq!(result.issues().len(), 1, "{:?}", result.issues());
    let issue = result
        .issues()
        .iter()
        .find(|issue| issue.path() == "Sensor.Reading")
        .expect("the reading is reported");
    assert_eq!(issue.kind(), &ProjectionIssueKind::MissingRequired);
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

/// A reading addressed through another resource is another signal.
///
/// `EnvironmentMetrics` reports a temperature the chassis also exposes as a
/// Sensor, and canonicalization drops the fragment naming the property, so the
/// key is that resource's own path. Nothing joins it to a Sensor descriptor:
/// treating it as one would attribute a reading to metadata describing a
/// different resource, and the sample has to come back for retry instead.
#[test]
fn a_metric_report_property_of_another_resource_resolves_to_no_sensor() {
    let sensor = fixture();
    let mut catalog = SignalCatalog::new();
    catalog
        .upsert(project::<SensorMetadataProjection>(&sensor))
        .expect("catalog capacity");

    let environment_metrics =
        SignalKey::from("/redfish/v1/Chassis/1/EnvironmentMetrics#/TemperatureCelsius");
    assert_eq!(
        environment_metrics.as_str(),
        "/redfish/v1/Chassis/1/EnvironmentMetrics",
        "the fragment names a property of the resource rather than a resource"
    );
    assert_eq!(
        catalog
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>(),
        ["/redfish/v1/Chassis/1/Sensors/CPU0Temp"],
        "the fixture catalogues one Sensor, under its own resource path"
    );
    assert!(catalog.get(&environment_metrics).is_none());

    let sample = SignalSample::new(
        "metric-report:ThermalReport",
        environment_metrics.clone(),
        finite(42.5),
    );
    let unresolved = catalog
        .resolve(sample)
        .expect_err("no Sensor descriptor claims another resource's property");

    assert_eq!(unresolved.key(), &environment_metrics);
    assert_eq!(
        unresolved.into_sample().signal_key(),
        &environment_metrics,
        "the rejected sample comes back for retry once metadata arrives"
    );
}

#[test]
fn a_status_the_device_reports_reaches_the_reading() {
    let sensor = fixture();
    let mut catalog = SignalCatalog::new();
    catalog
        .upsert(project::<SensorMetadataProjection>(&sensor))
        .expect("catalog capacity");

    let reading = catalog
        .resolve(project::<SensorSampleProjection>(&sensor))
        .expect("catalogued metadata");
    let reported = reading
        .reported_state
        .expect("the fixture reports Status.State and Status.Health");

    assert_eq!(
        reported.state.as_ref().map(OperatingState::as_str),
        Some("enabled")
    );
    assert_eq!(reported.health.as_ref().map(Health::as_str), Some("ok"));
}

#[test]
fn the_projected_resource_claims_only_the_properties_it_carries() {
    let record = project::<SensorResourceProjection>(&fixture());
    let resource = record.resource();

    assert_eq!(
        resource.completeness,
        ResourceCompleteness::Partial,
        "a property this projection does not lift is not a property the device lacks"
    );
    assert_eq!(resource.schema.as_ref().map(Name::as_str), Some("Sensor"));
    assert_eq!(resource.observed_at, Some(context().observed_at()));
}

#[test]
fn hostile_and_redundant_uri_spellings_name_one_signal() {
    let record = project::<SensorMetadataProjection>(&fixture());
    let key = record.key().clone();
    let subject = record.descriptor().subject.clone();
    let mut catalog = SignalCatalog::new();
    catalog.upsert(record).expect("catalog capacity");

    for spelling in [
        // A `://` inside a query string is not a scheme separator, so the
        // path before it is still the resource being addressed.
        "/redfish/v1/Chassis/1/Sensors/CPU0Temp?target=http://peer/x",
        "/redfish/v1//Chassis/1/Sensors//CPU0Temp/",
        "/redfish/v1/Chassis/1/Sensors/CPU%30Temp",
        "https://bmc.example/redfish/v1/Chassis/1/Sensors/CPU0Temp#/Reading",
        // A base concatenated onto a rooted path: the empty leading segment
        // does not move the sensor up a level.
        "//redfish/v1/Chassis/1/Sensors/CPU0Temp",
    ] {
        let mut json = fixture_json();
        json["@odata.id"] = json!(spelling);
        let record = project::<SensorMetadataProjection>(&sensor_from(json));

        assert_eq!(record.key(), &key, "{spelling}");
        assert_eq!(record.descriptor().subject, subject, "{spelling}");
        assert!(
            matches!(
                catalog.upsert(record).expect("catalog capacity"),
                SignalUpdate::Unchanged(_)
            ),
            "{spelling}"
        );
    }

    assert_eq!(
        catalog.len(),
        1,
        "one physical sensor holds one catalog entry"
    );
}

#[test]
fn an_empty_etag_is_no_version_rather_than_an_empty_one() {
    let mut json = fixture_json();
    json["@odata.etag"] = json!("");
    let record = project::<SensorResourceProjection>(&sensor_from(json));

    assert_eq!(
        record.resource().version,
        None,
        "an unusable version is indistinguishable from a real one downstream"
    );
}

#[test]
fn an_unusable_identity_is_reported_and_produces_nothing() {
    // An empty Id would otherwise scope every sensor in the chassis to the
    // same subject, and an empty @odata.id would give them all the signal
    // key `/`.
    for (field, path) in [("@odata.id", "Sensor.@odata.id"), ("Id", "Sensor.Id")] {
        let mut json = fixture_json();
        json[field] = json!("");
        let sensor = sensor_from(json);

        let metadata = SensorMetadataProjection::project(&sensor, &context());
        let resource = SensorResourceProjection::project(&sensor, &context());
        let sample = SensorSampleProjection::project(&sensor, &context());

        assert!(metadata.output().is_none(), "{field}");
        assert!(resource.output().is_none(), "{field}");
        let mut outputs = vec![metadata.issues(), resource.issues()];
        // The sample projection reads no Sensor.Id, so it survives an empty
        // one and only the identity it does read can fail.
        if field == "@odata.id" {
            assert!(sample.output().is_none(), "{field}");
            outputs.push(sample.issues());
        }
        for issues in outputs {
            assert!(
                issues.iter().any(|issue| issue.path() == path
                    && matches!(issue.kind(), ProjectionIssueKind::Invalid { .. })),
                "{field}: {issues:?}"
            );
        }
    }
}

#[test]
fn a_projected_source_key_re_keys_to_the_signal_it_already_names() {
    // `SignalKey::from(SourceKey)` is a no-op on a canonical key, so a
    // consumer that hands a projected source key back reaches the same
    // catalog entry rather than one that can never resolve.
    for spelling in [
        "/redfish/v1/Chassis/1/Sensors/%4%41",
        "/redfish/v1/Chassis/1/Sensors/%2%41",
        "/redfish/v1/Chassis/1/Sensors/%%41",
        "/redfish/v1/Chassis/1/Sensors/CPU0Temp%",
        "/redfish/v1/Chassis/1/Sensors/CPU%2fTemp",
    ] {
        let mut json = fixture_json();
        json["@odata.id"] = json!(spelling);
        let sensor = sensor_from(json);
        let mut catalog = SignalCatalog::new();
        catalog
            .upsert(project::<SensorMetadataProjection>(&sensor))
            .expect("catalog capacity");
        let sample = project::<SensorSampleProjection>(&sensor);

        let rekeyed = SignalSample::new(
            sample.source_key().clone(),
            sample.source_key().clone(),
            finite(42.5),
        );

        assert_eq!(rekeyed.signal_key(), sample.signal_key(), "{spelling}");
        catalog
            .resolve(rekeyed)
            .unwrap_or_else(|error| panic!("{spelling}: {error}"));
    }
}

#[test]
fn a_mangled_uri_does_not_collide_with_the_sensor_it_would_have_spelled() {
    let key = |odata_id: &str| {
        let mut json = fixture_json();
        json["@odata.id"] = json!(odata_id);
        project::<SensorMetadataProjection>(&sensor_from(json))
            .key()
            .clone()
    };

    // `%2A` is how a sensor named `*` is addressed. A device that dropped a
    // digit out of some other escape has not named that sensor.
    assert_ne!(
        key("/redfish/v1/Chassis/1/Sensors/%2%41"),
        key("/redfish/v1/Chassis/1/Sensors/%2A")
    );
}

#[test]
fn a_dot_segment_is_rejected_however_the_device_escaped_it() {
    for odata_id in [
        "/redfish/v1/Chassis/%2E%2E/Sensors/CPU0Temp",
        "/redfish/v1/Chassis/../Sensors/CPU0Temp",
    ] {
        let mut json = fixture_json();
        json["@odata.id"] = json!(odata_id);

        let result = SensorMetadataProjection::project(&sensor_from(json), &context());

        assert!(result.output().is_none(), "{odata_id}");
        assert!(
            result.issues().iter().any(|issue| {
                issue.path() == "Sensor.@odata.id"
                    && matches!(issue.kind(), ProjectionIssueKind::Invalid { .. })
            }),
            "{odata_id}: {:?}",
            result.issues()
        );
    }
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
