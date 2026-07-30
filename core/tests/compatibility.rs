// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

#![cfg(feature = "serde")]

use nv_telemetry_core::Finite;
use nv_telemetry_core::Origin;
use nv_telemetry_core::Retryable;
use nv_telemetry_core::Subject;
use nv_telemetry_core::ValueRange;
use serde_json::Value;

type LegacyCbor = (Retryable, Retryable, ValueRange, Subject, Origin);

#[test]
fn legacy_json_preserves_typed_wrappers_ranges_and_both_retry_values() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/legacy_serde.json")).expect("valid fixture");

    let yes: Retryable =
        serde_json::from_value(fixture["retryable_yes"].clone()).expect("legacy true");
    let no: Retryable =
        serde_json::from_value(fixture["retryable_no"].clone()).expect("legacy false");
    let range: ValueRange =
        serde_json::from_value(fixture["value_range"].clone()).expect("legacy range");
    let subject: Subject =
        serde_json::from_value(fixture["subject"].clone()).expect("legacy subject");
    let origin: Origin = serde_json::from_value(fixture["origin"].clone()).expect("legacy origin");

    assert_eq!((yes, no), (Retryable::Yes, Retryable::No));
    assert_eq!(range.lower(), Some(Finite::new(-5.0).unwrap()));
    assert_eq!(range.upper(), Some(Finite::new(100.0).unwrap()));
    assert_eq!(subject.kind.as_str(), "sensor");
    assert_eq!(subject.id.as_str(), "CPU0Temp");
    assert_eq!(origin.provider.as_str(), "redfish");
    assert_eq!(origin.request_class.as_str(), "sensor-reading");

    assert_eq!(serde_json::to_value(yes).unwrap(), fixture["retryable_yes"]);
    assert_eq!(serde_json::to_value(no).unwrap(), fixture["retryable_no"]);
    assert_eq!(serde_json::to_value(range).unwrap(), fixture["value_range"]);
    assert_eq!(serde_json::to_value(subject).unwrap(), fixture["subject"]);
    assert_eq!(serde_json::to_value(origin).unwrap(), fixture["origin"]);
}

#[test]
fn legacy_json_rejects_inverted_value_range() {
    let error = serde_json::from_str::<ValueRange>(include_str!(
        "fixtures/legacy_inverted_value_range.json"
    ))
    .expect_err("legacy inverted range must remain invalid");

    assert!(error
        .to_string()
        .contains("lower limit of 100 must not exceed upper limit of -5"));
}

#[test]
fn legacy_cbor_preserves_typed_wrappers_ranges_and_both_retry_values() {
    let bytes = decode_hex(include_str!("fixtures/legacy_serde.cbor.hex"));
    let values: LegacyCbor =
        ciborium::from_reader(bytes.as_slice()).expect("decode legacy CBOR fixture");

    assert_eq!(values.0, Retryable::Yes);
    assert_eq!(values.1, Retryable::No);
    assert_eq!(values.2.lower(), Some(Finite::new(-5.0).unwrap()));
    assert_eq!(values.2.upper(), Some(Finite::new(100.0).unwrap()));
    assert_eq!(values.3.kind.as_str(), "sensor");
    assert_eq!(values.3.id.as_str(), "CPU0Temp");
    assert_eq!(values.4.provider.as_str(), "redfish");
    assert_eq!(values.4.request_class.as_str(), "sensor-reading");

    let mut encoded = Vec::new();
    ciborium::into_writer(&values, &mut encoded).expect("encode legacy CBOR values");
    assert_eq!(encoded, bytes);
}

fn decode_hex(input: &str) -> Vec<u8> {
    let digits: Vec<_> = input.bytes().filter(u8::is_ascii_hexdigit).collect();
    assert_eq!(digits.len() % 2, 0, "fixture has a complete final byte");
    digits
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).expect("ASCII hex");
            u8::from_str_radix(pair, 16).expect("valid hexadecimal byte")
        })
        .collect()
}
