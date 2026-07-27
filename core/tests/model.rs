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
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use nv_telemetry_core::{
    AcquisitionOutcome, AcquisitionStatus, AttrValue, Attribute, Attributes, AttributesError,
    Completeness, Coverage, EndpointContext, FailureClass, Finite, Name, ObservationScope,
    ObservationWindow, Origin, Subject, Timestamp, TimestampError,
};

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
fn timestamps_validate_and_round_trip_before_the_epoch() {
    assert_eq!(
        Timestamp::new(0, Timestamp::NANOS_PER_SECOND),
        Err(TimestampError::InvalidNanoseconds(
            Timestamp::NANOS_PER_SECOND
        ))
    );
    assert_eq!(
        ObservationWindow::new(timestamp(2), timestamp(1)),
        Err(TimestampError::WindowEndsBeforeStart)
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
fn non_finite_values_are_rejected_at_construction() {
    assert!(Finite::new(f64::NAN).is_err());
    assert!(Finite::new(f64::INFINITY).is_err());
    assert!(AttrValue::f64(f64::NEG_INFINITY).is_err());

    // Negative zero is normalized so that equality and hashing agree.
    assert_eq!(
        Finite::new(-0.0).expect("finite"),
        Finite::new(0.0).expect("finite")
    );
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
fn serde_round_trip_preserves_acquisition_status() {
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
fn serde_rejects_invalid_invariant_bearing_values() {
    let bad_timestamp = r#"{"seconds":0,"nanoseconds":1000000000}"#;
    assert!(serde_json::from_str::<Timestamp>(bad_timestamp).is_err());

    let duplicate_attributes =
        r#"[{"key":"rack","value":{"string":"one"}},{"key":"rack","value":{"string":"two"}}]"#;
    assert!(serde_json::from_str::<Attributes>(duplicate_attributes).is_err());

    let non_finite_attribute = r#"{"f64":null}"#;
    assert!(serde_json::from_str::<AttrValue>(non_finite_attribute).is_err());
}
