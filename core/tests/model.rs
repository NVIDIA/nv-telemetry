// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::Arc;
use std::time::Duration;
use std::time::UNIX_EPOCH;

use nv_telemetry_core::finite;
use nv_telemetry_core::AcquisitionOutcome;
use nv_telemetry_core::AcquisitionStatus;
use nv_telemetry_core::AttrValue;
use nv_telemetry_core::Attribute;
use nv_telemetry_core::Attributes;
use nv_telemetry_core::AttributesError;
use nv_telemetry_core::Completeness;
use nv_telemetry_core::Coverage;
use nv_telemetry_core::DurationValue;
use nv_telemetry_core::EndpointContext;
use nv_telemetry_core::FailureClass;
use nv_telemetry_core::Finite;
use nv_telemetry_core::InventoryBuilder;
use nv_telemetry_core::InventoryItem;
use nv_telemetry_core::LogRecord;
use nv_telemetry_core::LogsBuilder;
use nv_telemetry_core::Name;
use nv_telemetry_core::NumericValue;
use nv_telemetry_core::ObservationBatch;
use nv_telemetry_core::ObservationScope;
use nv_telemetry_core::ObservationWindow;
use nv_telemetry_core::ObservedResource;
use nv_telemetry_core::Origin;
use nv_telemetry_core::Payload;
use nv_telemetry_core::PropertyMap;
use nv_telemetry_core::Reading;
use nv_telemetry_core::ReadingKind;
use nv_telemetry_core::ReadingsBuilder;
use nv_telemetry_core::ReportedState;
use nv_telemetry_core::ResourceGraph;
use nv_telemetry_core::Severity;
use nv_telemetry_core::SignalDescriptor;
use nv_telemetry_core::StateObservation;
use nv_telemetry_core::StatesBuilder;
use nv_telemetry_core::Subject;
use nv_telemetry_core::TimeError;
use nv_telemetry_core::Timestamp;
use nv_telemetry_core::Unit;
use nv_telemetry_core::ValueRange;

fn timestamp(seconds: i64) -> Timestamp {
    Timestamp::new(seconds, 0).expect("valid timestamp")
}

fn endpoint() -> Arc<EndpointContext> {
    Arc::new(EndpointContext::new(
        "bmc-00:11:22:33:44:55",
        Attributes::new(vec![
            Attribute::new("device_class", "compute_node"),
            Attribute::new("rack", "rack-1"),
        ])
        .expect("unique attributes"),
    ))
}

fn origin() -> Origin {
    Origin::new("redfish-sensor-odata", "redfish-sensor-odata")
}

fn window() -> ObservationWindow {
    ObservationWindow::new(timestamp(100), timestamp(101)).expect("ordered window")
}

fn sensor_subject() -> Subject {
    Subject::new("sensor", "/redfish/v1/Chassis/1/Sensors/CPU0Temp")
}

fn hash_of<T: Hash>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn descriptor() -> SignalDescriptor {
    SignalDescriptor::new(
        sensor_subject(),
        "temperature",
        "CPU0Temp",
        ReadingKind::Gauge,
        Unit::from_static("celsius"),
        timestamp(99),
    )
}

fn reading() -> Reading {
    Reading::new(
        "/redfish/v1/Chassis/1/Sensors/CPU0Temp",
        descriptor(),
        finite!(42.5),
    )
}

#[cfg(feature = "serde")]
fn readings_batch(rows: Vec<Reading>) -> ObservationBatch {
    ObservationBatch::new(
        endpoint(),
        origin(),
        window(),
        Coverage::complete_subject(sensor_subject()),
        Payload::Readings(rows.into_boxed_slice()),
    )
    .expect("valid batch")
}

#[test]
fn static_and_shared_names_compare_and_hash_by_content() {
    let static_name = Name::from_static("temperature");
    let shared_name = Name::from(String::from("temperature"));

    assert!(static_name.is_static());
    assert!(!shared_name.is_static());
    assert_eq!(static_name, shared_name);

    let mut static_hasher = DefaultHasher::new();
    static_name.hash(&mut static_hasher);
    let mut shared_hasher = DefaultHasher::new();
    shared_name.hash(&mut shared_hasher);
    assert_eq!(static_hasher.finish(), shared_hasher.finish());
}

/// Plain records expose their fields; invariant carriers keep theirs private.
///
/// This test lives outside the crate on purpose. Field privacy is scoped to the
/// defining module, so a unit test would see every field regardless and could
/// not tell the two kinds of type apart.
#[test]
fn plain_records_read_and_match_as_data() {
    let reading = reading();

    let Reading {
        value,
        signal,
        reported_state,
        ..
    } = &reading;

    assert_eq!(*value, NumericValue::F64(finite!(42.5)));
    assert_eq!(signal.subject.kind.as_str(), "sensor");
    assert_eq!(signal.unit.as_str(), "celsius");
    assert!(reported_state.is_none());

    let attributes = Attributes::new(vec![Attribute::new("b", 1_u64), Attribute::new("a", 2_u64)])
        .expect("unique attributes");
    assert_eq!(attributes.as_slice()[0].key.as_str(), "a");
}

#[test]
fn attributes_are_sorted_searchable_and_unique() {
    let attributes = Attributes::new(vec![Attribute::new("z", 3_u64), Attribute::new("a", true)])
        .expect("unique attributes");

    assert_eq!(attributes.as_slice()[0].key.as_str(), "a");
    assert_eq!(attributes.get("z"), Some(&AttrValue::U64(3)));

    let duplicate = Attributes::new(vec![
        Attribute::new("rack", "one"),
        Attribute::new("rack", "two"),
    ]);
    assert!(matches!(
        duplicate,
        Err(AttributesError::DuplicateKey(key)) if key.as_str() == "rack"
    ));
}

#[test]
fn equal_attributes_hash_equally_however_they_were_built() {
    // Each collection digests its entries once and hashes the digest, so two
    // built separately have to agree for `Hash` to stay consistent with `Eq`.
    let sorted = Attributes::new(vec![Attribute::new("a", true), Attribute::new("z", 3_u64)])
        .expect("unique attributes");
    let reversed = Attributes::new(vec![Attribute::new("z", 3_u64), Attribute::new("a", true)])
        .expect("unique attributes");
    let different = Attributes::new(vec![Attribute::new("a", false), Attribute::new("z", 3_u64)])
        .expect("unique attributes");

    assert_eq!(sorted, reversed);
    assert_eq!(hash_of(&sorted), hash_of(&reversed));
    assert_ne!(sorted, different);
    assert_ne!(hash_of(&sorted), hash_of(&different));

    assert_eq!(Attributes::empty(), Attributes::empty());
    assert_eq!(hash_of(&Attributes::empty()), hash_of(&Attributes::empty()));
}

#[test]
fn timestamps_validate_and_round_trip_before_the_epoch() {
    assert_eq!(
        Timestamp::new(0, Timestamp::NANOS_PER_SECOND),
        Err(TimeError::InvalidNanoseconds(Timestamp::NANOS_PER_SECOND))
    );
    assert_eq!(
        ObservationWindow::new(timestamp(2), timestamp(1)),
        Err(TimeError::WindowEndsBeforeStart)
    );

    let system_time = UNIX_EPOCH - Duration::from_millis(500);
    let timestamp = Timestamp::from_system_time(system_time).expect("representable timestamp");
    assert_eq!(timestamp.seconds(), -1);
    assert_eq!(timestamp.nanoseconds(), 500_000_000);
    assert_eq!(
        timestamp
            .to_system_time()
            .expect("representable system time"),
        system_time
    );
}

#[test]
fn durations_share_the_timestamp_bound_and_carry_sign_in_the_seconds() {
    assert!(DurationValue::new(1, 999_999_999).is_ok());
    assert_eq!(
        DurationValue::new(1, DurationValue::NANOS_PER_SECOND),
        Err(TimeError::InvalidNanoseconds(
            DurationValue::NANOS_PER_SECOND
        ))
    );

    let negative_half_second = DurationValue::new(-1, 500_000_000).expect("valid duration");
    assert_eq!(negative_half_second.as_nanos(), -500_000_000);
    assert!(negative_half_second < DurationValue::new(0, 0).expect("valid duration"));
}

#[test]
fn all_payload_domains_build_as_immutable_slices() {
    let mut readings = ReadingsBuilder::new();
    readings.push(reading());
    assert_eq!(readings.len(), 1);

    let mut logs = LogsBuilder::new();
    logs.push(
        LogRecord::new(Severity::from_static("warning"), "fan degraded")
            .with_record_id("log-1")
            .with_subject(sensor_subject()),
    );

    let mut states = StatesBuilder::new();
    states.push(
        StateObservation::new("power_state", "on")
            .with_instance("system-1")
            .with_subject(Subject::new("computer_system", "system-1")),
    );

    let mut inventory = InventoryBuilder::new();
    inventory.push(InventoryItem::new(sensor_subject(), Attributes::empty()));

    let resources = ResourceGraph::new(
        vec![ObservedResource::complete(
            sensor_subject(),
            "/redfish/v1/Chassis/1/Sensors/CPU0Temp",
            PropertyMap::empty(),
        )],
        Vec::new(),
    )
    .expect("valid resource graph");

    let payloads = [
        Payload::Readings(readings.finish()),
        Payload::Logs(logs.finish()),
        Payload::States(states.finish()),
        Payload::Inventory(inventory.finish()),
        Payload::Resources(resources),
    ];
    assert!(payloads.iter().all(|payload| payload.len() == 1));
}

#[test]
fn coverage_pairs_a_scope_with_the_claim_made_about_it() {
    let subject = Subject::new("sensor", "/redfish/v1/Chassis/1/Sensors/CPU0Temp");
    let complete = Coverage::complete_subject(subject.clone());
    assert_eq!(complete.scope, ObservationScope::Subject(subject));
    assert!(complete.completeness.is_complete());

    let partial = Coverage::new(ObservationScope::Endpoint, Completeness::partial(Some(2)));
    assert_eq!(partial.scope, ObservationScope::Endpoint);
    assert!(!partial.completeness.is_complete());
}

#[test]
fn scope_is_enforced_and_completeness_is_relative_to_it() {
    let sensor = sensor_subject();
    let other = Subject::new("sensor", "other");
    let batch = ObservationBatch::new(
        endpoint(),
        origin(),
        window(),
        Coverage::complete_subject(sensor.clone()),
        Payload::Readings(vec![reading()].into_boxed_slice()),
    )
    .expect("reading is inside sensor scope");
    assert_eq!(batch.coverage().scope, ObservationScope::Subject(sensor));

    let mismatch = ObservationBatch::new(
        endpoint(),
        origin(),
        window(),
        Coverage::complete_subject(other),
        Payload::Readings(vec![reading()].into_boxed_slice()),
    );
    assert!(mismatch.is_err());

    let partial = Coverage::new(ObservationScope::Endpoint, Completeness::partial(Some(2)));
    assert!(!partial.completeness.is_complete());
}

#[test]
fn every_payload_domain_is_scope_checked_the_same_way() {
    let sensor = sensor_subject();
    let other = Subject::new("sensor", "other");

    let scoped = |payload| {
        ObservationBatch::new(
            endpoint(),
            origin(),
            window(),
            Coverage::complete_subject(sensor.clone()),
            payload,
        )
    };

    let log = |subject: Subject| {
        Payload::Logs(
            vec![
                LogRecord::new(Severity::from_static("warning"), "fan degraded")
                    .with_subject(subject),
            ]
            .into_boxed_slice(),
        )
    };
    assert!(scoped(log(sensor.clone())).is_ok());
    assert!(scoped(log(other.clone())).is_err());

    let state = |subject: Subject| {
        Payload::States(
            vec![StateObservation::new("power_state", "on").with_subject(subject)]
                .into_boxed_slice(),
        )
    };
    assert!(scoped(state(sensor.clone())).is_ok());
    assert!(scoped(state(other.clone())).is_err());

    let item = |subject: Subject| {
        Payload::Inventory(
            vec![InventoryItem::new(subject, Attributes::empty())].into_boxed_slice(),
        )
    };
    assert!(scoped(item(sensor.clone())).is_ok());
    assert!(scoped(item(other)).is_err());
}

#[test]
fn a_row_without_a_subject_belongs_to_any_scope() {
    let unattributed = LogRecord::new(Severity::from_static("warning"), "psu noise");
    assert!(unattributed.subject.is_none());

    let batch = ObservationBatch::new(
        endpoint(),
        origin(),
        window(),
        Coverage::complete_subject(sensor_subject()),
        Payload::Logs(vec![unattributed].into_boxed_slice()),
    );
    assert!(batch.is_ok());
}

/// Shared ownership is a representation choice, not a demand on the caller.
///
/// A projection holding a catalog passes the handle it already has; a caller
/// building a single observation passes the value. Both must reach the same
/// batch, or the convenience is a second code path rather than a shorthand.
#[test]
fn owned_values_and_shared_handles_build_the_same_batch() {
    let descriptor = SignalDescriptor::new(
        sensor_subject(),
        "temperature",
        "CPU0Temp",
        ReadingKind::Gauge,
        Unit::from_static("celsius"),
        timestamp(99),
    );
    let source_key = "/redfish/v1/Chassis/1/Sensors/CPU0Temp";

    let from_value = Reading::new(source_key, descriptor.clone(), finite!(42.5));
    let from_handle = Reading::new(source_key, Arc::new(descriptor), finite!(42.5));
    assert_eq!(from_value, from_handle);

    let context = EndpointContext::new("bmc-1", Attributes::empty());
    let owned = ObservationBatch::new(
        context.clone(),
        origin(),
        window(),
        Coverage::complete_endpoint(),
        Payload::Readings(vec![from_value].into_boxed_slice()),
    )
    .expect("valid batch");
    let shared = ObservationBatch::new(
        Arc::new(context),
        origin(),
        window(),
        Coverage::complete_endpoint(),
        Payload::Readings(vec![from_handle].into_boxed_slice()),
    )
    .expect("valid batch");
    assert_eq!(owned, shared);

    let status = AcquisitionStatus::new(
        Arc::clone(&owned.endpoint),
        origin(),
        window(),
        AcquisitionOutcome::succeeded(1),
    );
    assert_eq!(status.endpoint.id, owned.endpoint.id);
    assert!(Arc::ptr_eq(&status.endpoint, &owned.endpoint));
}

#[test]
fn equal_batches_hash_equally_so_observations_can_be_content_addressed() {
    let build = || {
        ObservationBatch::new(
            endpoint(),
            origin(),
            window(),
            Coverage::complete_subject(sensor_subject()),
            Payload::Readings(vec![reading()].into_boxed_slice()),
        )
        .expect("valid batch")
    };

    let first = build();
    let second = build();
    assert_eq!(first, second);
    assert_eq!(hash_of(&first), hash_of(&second));

    let different = ObservationBatch::new(
        endpoint(),
        origin(),
        window(),
        Coverage::complete_endpoint(),
        Payload::Inventory(Box::new([])),
    )
    .expect("valid batch");
    assert_ne!(hash_of(&first), hash_of(&different));
}

#[test]
fn complete_empty_inventory_snapshot_is_valid_and_shareable() {
    let batch = ObservationBatch::new(
        endpoint(),
        origin(),
        window(),
        Coverage::complete_endpoint(),
        Payload::Inventory(Box::new([])),
    )
    .expect("empty complete inventory is meaningful");

    let shared = Arc::new(batch);
    let second_consumer = Arc::clone(&shared);
    assert!(Arc::ptr_eq(&shared, &second_consumer));
    assert!(shared.payload().is_empty());
}

#[test]
fn sensor_inventory_and_reading_share_subject_and_health_context() {
    let entity = Subject::new("processor", "/redfish/v1/Systems/1/Processors/CPU0");
    let sensor = sensor_subject();
    let inventory_item = InventoryItem::new(
        sensor.clone(),
        Attributes::new(vec![
            Attribute::new("entity_type", "processor"),
            Attribute::new("physical_context", "cpu"),
            Attribute::new("sensor_name", "CPU0Temp"),
        ])
        .expect("unique inventory attributes"),
    )
    .with_parent(entity);

    let signal = Arc::new(
        SignalDescriptor::new(
            sensor.clone(),
            "temperature",
            "CPU0Temp",
            ReadingKind::Gauge,
            Unit::from_static("celsius"),
            timestamp(99),
        )
        .with_bounds(ValueRange::new(Some(finite!(-5.0)), Some(finite!(100.0)))),
    );
    let reading = Reading::new(
        "/redfish/v1/Chassis/1/Sensors/CPU0Temp",
        signal,
        finite!(42.5),
    )
    .with_reported_state(ReportedState::new(
        Some(Name::from_static("enabled")),
        Some(Name::from_static("critical")),
    ));

    assert_eq!(inventory_item.subject, reading.signal.subject);
    assert_eq!(
        inventory_item.attributes.get("physical_context"),
        Some(&AttrValue::String(Name::from_static("cpu")))
    );
    assert_eq!(
        reading
            .reported_state
            .as_ref()
            .and_then(|state| state.health.as_ref())
            .map(Name::as_str),
        Some("critical")
    );
    assert_eq!(
        reading.signal.bounds.and_then(|bounds| bounds.upper),
        Some(finite!(100.0))
    );
    assert_eq!(reading.value, NumericValue::F64(finite!(42.5)));
}

#[test]
fn reported_ranges_are_stored_permissively_but_can_be_checked() {
    let inverted = ValueRange::new(Some(finite!(100.0)), Some(finite!(-5.0)));

    // Storage keeps what the device said; `checked` is the opt-in gate.
    assert_eq!(inverted.lower, Some(finite!(100.0)));

    let error = inverted.checked().expect_err("the edges contradict");
    assert_eq!(error.lower, finite!(100.0));
    assert_eq!(error.upper, finite!(-5.0));

    assert!(ValueRange::new(Some(finite!(-5.0)), None).checked().is_ok());
    assert!(ValueRange::empty().checked().is_ok());
}

#[test]
fn non_finite_values_are_rejected_at_construction() {
    assert!(Finite::new(f64::NAN).is_err());
    assert!(NumericValue::f64(f64::INFINITY).is_err());
    assert!(AttrValue::f64(f64::NEG_INFINITY).is_err());
}

#[test]
fn acquisition_failure_is_status_not_an_observation() {
    let status = AcquisitionStatus::new(
        endpoint(),
        origin(),
        window(),
        AcquisitionOutcome::failed(FailureClass::Timeout, true),
    );

    assert!(status.outcome.retryable());
    assert!(matches!(
        status.outcome,
        AcquisitionOutcome::Failed {
            class: FailureClass::Timeout,
            retryable: true
        }
    ));
}

#[cfg(feature = "serde")]
#[test]
fn serde_round_trip_preserves_validated_batch_and_status() {
    let batch = ObservationBatch::new(
        endpoint(),
        origin(),
        window(),
        Coverage::complete_subject(sensor_subject()),
        Payload::Readings(vec![reading()].into_boxed_slice()),
    )
    .expect("valid batch");
    let encoded = serde_json::to_string(&batch).expect("serialize batch");
    let decoded: ObservationBatch = serde_json::from_str(&encoded).expect("deserialize batch");
    assert_eq!(decoded, batch);

    let status = AcquisitionStatus::new(
        endpoint(),
        origin(),
        window(),
        AcquisitionOutcome::succeeded(1),
    );
    let encoded = serde_json::to_string(&status).expect("serialize status");
    let decoded: AcquisitionStatus = serde_json::from_str(&encoded).expect("deserialize status");
    assert_eq!(decoded, status);
}

#[cfg(feature = "serde")]
#[test]
fn readings_share_one_descriptor_across_the_wire() {
    let shared = Arc::new(descriptor());
    let rows: Vec<Reading> = (0..3)
        .map(|index| {
            Reading::new(
                format!("sample-{index}"),
                Arc::clone(&shared),
                finite!(42.5),
            )
        })
        .collect();
    let batch = readings_batch(rows);

    // The descriptor is written once, not once per row.
    let encoded = serde_json::to_string(&batch).expect("serialize batch");
    assert_eq!(encoded.matches("celsius").count(), 1);

    let decoded: ObservationBatch = serde_json::from_str(&encoded).expect("deserialize batch");
    assert_eq!(decoded, batch);
    let Payload::Readings(decoded_rows) = decoded.payload() else {
        panic!("expected readings");
    };
    assert!(Arc::ptr_eq(
        &decoded_rows[0].signal,
        &decoded_rows[2].signal
    ));
}

#[cfg(feature = "serde")]
#[test]
fn descriptor_sharing_is_a_memory_detail_the_wire_format_does_not_expose() {
    // Equal batches, one sharing a descriptor and one holding three copies.
    let shared = Arc::new(descriptor());
    let with_sharing = readings_batch(
        (0..3)
            .map(|index| {
                Reading::new(
                    format!("sample-{index}"),
                    Arc::clone(&shared),
                    finite!(42.5),
                )
            })
            .collect(),
    );
    let without_sharing = readings_batch(
        (0..3)
            .map(|index| Reading::new(format!("sample-{index}"), descriptor(), finite!(42.5)))
            .collect(),
    );
    assert_eq!(with_sharing, without_sharing);

    let encoded = serde_json::to_string(&with_sharing).expect("serialize shared");
    assert_eq!(
        encoded,
        serde_json::to_string(&without_sharing).expect("serialize unshared")
    );

    let decoded: ObservationBatch = serde_json::from_str(&encoded).expect("deserialize batch");
    let Payload::Readings(rows) = decoded.payload() else {
        panic!("expected readings");
    };
    assert!(Arc::ptr_eq(&rows[0].signal, &rows[2].signal));
}

#[cfg(feature = "serde")]
#[test]
fn serde_rejects_a_reading_naming_a_signal_the_table_does_not_hold() {
    let batch = readings_batch(vec![reading()]);
    let encoded = serde_json::to_string(&batch).expect("serialize batch");
    let dangling = encoded.replace(r#""signal":0"#, r#""signal":7"#);
    assert_ne!(dangling, encoded);
    assert!(serde_json::from_str::<ObservationBatch>(&dangling).is_err());
}

#[cfg(feature = "serde")]
#[test]
fn serde_rejects_invalid_invariant_bearing_values() {
    let bad_timestamp = r#"{"seconds":0,"nanoseconds":1000000000}"#;
    assert!(serde_json::from_str::<Timestamp>(bad_timestamp).is_err());

    // `DurationValue` shares the bound with `Timestamp` and validates on the
    // way in for the same reason.
    let bad_duration = r#"{"seconds":0,"nanoseconds":1000000000}"#;
    assert!(serde_json::from_str::<DurationValue>(bad_duration).is_err());
    let duration = DurationValue::new(-3, 500).expect("a valid duration");
    assert_eq!(
        serde_json::from_str::<DurationValue>(
            &serde_json::to_string(&duration).expect("serialize duration")
        )
        .expect("round trips"),
        duration
    );

    let duplicate_attributes =
        r#"[{"key":"rack","value":{"string":"one"}},{"key":"rack","value":{"string":"two"}}]"#;
    assert!(serde_json::from_str::<Attributes>(duplicate_attributes).is_err());
}
