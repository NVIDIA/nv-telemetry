// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Properties of the generated wire types.
//!
//! Unit tests rather than integration tests, because the generated module is
//! crate-internal: making it public so a `tests/` file could reach it would
//! trade the guarantee for the convenience of testing it.
//!
//! These check the schema's decisions as they actually reach a consumer, not
//! as they read in the `.proto`. The presence discipline in particular is
//! only worth anything if it survives into the generated API — a scalar that
//! arrived as a bare `f64` instead of an `Option<f64>` would mean an absent
//! reading and a reading of zero had become the same value again.

use prost::Message as _;
use prost_reflect::DescriptorPool;
use prost_reflect::DynamicMessage;
use prost_reflect::Value as V;

use crate::generated::limits;
use crate::generated::wire;
use crate::Finite;
use crate::NumericValue;
use crate::Timestamp;
use crate::Value;
use crate::ValueKind;
use crate::Violation;

#[test]
fn an_absent_reading_and_a_zero_reading_are_different_bytes() {
    let absent = wire::NumericValue { kind: None };
    let zero = wire::NumericValue {
        kind: Some(wire::numeric_value::Kind::DoubleValue(0.0)),
    };

    assert_ne!(absent.encode_to_vec(), zero.encode_to_vec());

    // And they survive the round trip as different values, which is the whole
    // point: a device that did not answer must not decode as a device that
    // answered zero.
    let decoded = wire::NumericValue::decode(zero.encode_to_vec().as_slice()).unwrap();
    assert_eq!(decoded, zero);
    assert_ne!(decoded, absent);
}

#[test]
fn explicit_presence_survives_into_the_generated_api() {
    // Every scalar the schema declared `optional` must be an Option here. If
    // prost ever lowered one to a bare value, absence would stop being
    // representable and the presence rule would be enforced only on paper.
    let empty = wire::Subject::default();
    assert!(empty.kind.is_none(), "Subject.kind lost explicit presence");
    assert!(empty.id.is_none(), "Subject.id lost explicit presence");

    let empty = wire::Timestamp::default();
    assert!(empty.seconds.is_none());
    assert!(
        empty.nanos.is_none(),
        "Timestamp.nanos lost explicit presence"
    );

    let empty = wire::ObservedResource::default();
    assert!(
        empty.properties_complete.is_none(),
        "properties_complete lost explicit presence; absent would read as false"
    );

    // Enum-typed fields are the highest-risk case: a proto3 enum that loses
    // `optional` becomes a bare i32 where 0 is UNSPECIFIED, which collapses
    // "the device did not say" into "the device said unspecified".
    assert!(wire::Coverage::default().completeness.is_none());
    assert!(wire::LogRecord::default().severity.is_none());
    let status = wire::AcquisitionStatus::default();
    assert!(status.outcome.is_none());
    assert!(status.failure_class.is_none());
}

#[test]
fn an_unknown_enum_value_survives_rather_than_collapsing() {
    // proto3 enums are open. A newer producer's completeness value must reach
    // a consumer intact so it can refuse to interpret it, rather than being
    // silently rewritten to UNSPECIFIED — which reads as "did not say".
    let ahead = wire::Coverage {
        completeness: Some(99),
        scope: None,
    };
    let decoded = wire::Coverage::decode(ahead.encode_to_vec().as_slice()).unwrap();

    assert_eq!(decoded.completeness, Some(99));
    assert!(wire::Completeness::try_from(99).is_err());
}

#[test]
fn unknown_fields_are_dropped_on_re_encode() {
    // prost keeps no unknown-field storage. A component built on these types
    // cannot forward a newer producer's fields — it silently strips them. The
    // evolution gate is additive-only, so this is the substrate's cost, and it
    // is pinned here so the behaviour is a decision rather than a surprise.
    let known = wire::Subject {
        kind: Some("sensor".into()),
        scope: vec![],
        id: Some("x".into()),
    };
    let mut bytes = known.encode_to_vec();
    // Field 99, varint, value 1 — as a future revision might add.
    bytes.extend_from_slice(&[0xF8, 0x06, 0x01]);

    let decoded = wire::Subject::decode(bytes.as_slice()).expect("forward-compatible decode");
    assert_eq!(decoded, known);
    assert_eq!(
        decoded.encode_to_vec(),
        known.encode_to_vec(),
        "the unknown field was retained; this test's premise changed"
    );
    assert!(decoded.encode_to_vec().len() < bytes.len());
}

#[test]
fn a_batch_round_trips_byte_for_byte() {
    let batch = wire::ObservationBatch {
        endpoint: Some(wire::EndpointContext {
            endpoint_id: Some("bmc-lab-07".into()),
            attributes: None,
        }),
        origin: Some(wire::Origin {
            provider: Some("redfish.sensor.odata".into()),
            request_class: Some("sensor-read".into()),
        }),
        window: Some(wire::ObservationWindow {
            start: Some(wire::Timestamp {
                seconds: Some(1_785_621_243),
                nanos: Some(0),
            }),
            end: None,
        }),
        coverage: Some(wire::Coverage {
            completeness: Some(wire::Completeness::Partial as i32),
            scope: None,
        }),
        payload: Some(wire::observation_batch::Payload::Readings(wire::Readings {
            descriptors: vec![wire::SignalDescriptor {
                key: Some(signal_key()),
                kind: Some("temperature".into()),
                unit: Some("Cel".into()),
                range: None,
            }],
            samples: vec![wire::Reading {
                key: Some(signal_key()),
                value: Some(wire::NumericValue {
                    kind: Some(wire::numeric_value::Kind::DoubleValue(47.5)),
                }),
                observed_at: None,
            }],
        })),
    };

    let encoded = batch.encode_to_vec();
    let decoded = wire::ObservationBatch::decode(encoded.as_slice()).expect("batch decodes");

    assert_eq!(decoded, batch);
    assert_eq!(
        decoded.encode_to_vec(),
        encoded,
        "re-encoding is not stable"
    );
}

#[test]
fn a_counter_above_two_to_the_fifty_third_survives() {
    // The reason NumericValue is a union rather than a bare double: this
    // value is exact as a uint64 and is not representable as an f64.
    let big = 91_827_364_554_433_777_u64;
    // The lossy round trip is the point: it asserts the constant genuinely
    // does not survive a double, so this test cannot quietly stop proving
    // anything if someone later picks a smaller one.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let through_double = big as f64 as u64;
    assert_ne!(through_double, big, "the test value must not fit a double");

    let value = wire::NumericValue {
        kind: Some(wire::numeric_value::Kind::UintValue(big)),
    };
    let decoded = wire::NumericValue::decode(value.encode_to_vec().as_slice()).unwrap();

    assert_eq!(
        decoded.kind,
        Some(wire::numeric_value::Kind::UintValue(big)),
        "a 64-bit counter lost precision on the wire"
    );
}

fn signal_key() -> wire::SignalKey {
    wire::SignalKey {
        subject: Some(wire::Subject {
            kind: Some("sensor".into()),
            scope: vec!["1U".into()],
            id: Some("CPU1Temp".into()),
        }),
        facet: None,
    }
}

#[test]
fn generated_types_and_descriptors_encode_identically() {
    let pool = pool();

    // Built with the generated types.
    let native = wire::Subject {
        kind: Some("sensor".into()),
        scope: vec!["1U".into(), "PSU1".into()],
        id: Some("CPU1Temp".into()),
    };

    // The same value, built against the descriptors the rules ran over.
    let descriptor = pool
        .get_message_by_name("nv.telemetry.v1.Subject")
        .expect("Subject is in the shipped schema");
    let mut dynamic = DynamicMessage::new(descriptor.clone());
    dynamic.set_field_by_name("kind", V::String("sensor".into()));
    dynamic.set_field_by_name(
        "scope",
        V::List(vec![V::String("1U".into()), V::String("PSU1".into())]),
    );
    dynamic.set_field_by_name("id", V::String("CPU1Temp".into()));

    assert_eq!(
        native.encode_to_vec(),
        dynamic.encode_to_vec(),
        "the generated type and the descriptors disagree about the wire format"
    );

    // And across the boundary in both directions.
    let from_dynamic = wire::Subject::decode(dynamic.encode_to_vec().as_slice())
        .expect("descriptor-encoded bytes decode into the generated type");
    assert_eq!(from_dynamic, native);

    DynamicMessage::decode(descriptor, native.encode_to_vec().as_slice())
        .expect("generated-encoded bytes decode against the descriptors");
}

#[test]
fn oneof_arms_carry_the_field_numbers_the_schema_declares() {
    let pool = pool();
    let descriptor = pool
        .get_message_by_name("nv.telemetry.v1.NumericValue")
        .expect("NumericValue is in the shipped schema");

    // Arm selection is a wire-level fact: picking the wrong tag would silently
    // reinterpret an integer counter as a float.
    for (name, native, value) in [
        (
            "uint_value",
            wire::NumericValue {
                kind: Some(wire::numeric_value::Kind::UintValue(91_827_364_554_433_777)),
            },
            V::U64(91_827_364_554_433_777),
        ),
        (
            "int_value",
            wire::NumericValue {
                kind: Some(wire::numeric_value::Kind::IntValue(-42)),
            },
            V::I64(-42),
        ),
    ] {
        let mut dynamic = DynamicMessage::new(descriptor.clone());
        dynamic.set_field_by_name(name, value);
        assert_eq!(
            native.encode_to_vec(),
            dynamic.encode_to_vec(),
            "generated `{name}` does not match the descriptor's encoding"
        );
    }
}

/// The descriptors the invariant rules ran over, decoded from the schema crate.
///
/// Built here rather than borrowed from the compiler so that this crate's
/// tests do not depend on it — the dependency runs the other way.
fn pool() -> DescriptorPool {
    DescriptorPool::decode(nv_telemetry_schema::DESCRIPTOR_SET).expect("shipped schema decodes")
}

#[test]
fn finite_refuses_what_would_break_total_equality() {
    assert!(Finite::new(f64::NAN).is_none());
    assert!(Finite::new(f64::INFINITY).is_none());
    assert!(Finite::new(f64::NEG_INFINITY).is_none());

    // The two zeros collapse to one representation, so equal values cannot
    // hash unequal.
    let positive = Finite::new(0.0).unwrap();
    let negative = Finite::new(-0.0).unwrap();
    assert_eq!(positive, negative);
    assert_eq!(positive.get().to_bits(), negative.get().to_bits());
}

#[test]
fn a_timestamp_carries_its_second_in_one_representation() {
    assert!(Timestamp::new(1_722_000_000, 999_999_999).is_ok());

    let overflow = Timestamp::new(1_722_000_000, 1_000_000_000).unwrap_err();
    assert_eq!(overflow.path(), "nanos");

    // The wire form round-trips through the same constructor, so a decoded
    // timestamp passes exactly what a built one does.
    let wire = wire::Timestamp {
        seconds: Some(5),
        nanos: None,
    };
    let error = Timestamp::try_from(wire).unwrap_err();
    assert_eq!(error.violation(), &Violation::Absent);
}

#[test]
fn a_numeric_value_refuses_a_fabricated_reading() {
    let error = NumericValue::double(f64::NAN).unwrap_err();
    assert_eq!(error.violation(), &Violation::NotFinite);

    let absent = wire::NumericValue { kind: None };
    let error = NumericValue::try_from(absent).unwrap_err();
    assert_eq!(error.path(), "kind");
    assert_eq!(error.violation(), &Violation::Absent);
}

#[test]
fn value_bounds_are_the_schemas_bounds() {
    let oversize = "x".repeat(limits::VALUE_STRING_VALUE_MAX_LEN as usize + 1);
    let error = Value::string(oversize).unwrap_err();
    assert_eq!(
        error.violation(),
        &Violation::TooLong {
            limit: limits::VALUE_STRING_VALUE_MAX_LEN,
            actual: limits::VALUE_STRING_VALUE_MAX_LEN as usize + 1,
        }
    );

    let at_bound = "x".repeat(limits::VALUE_STRING_VALUE_MAX_LEN as usize);
    assert!(Value::string(at_bound).is_ok());
}

#[test]
fn a_map_rejects_a_duplicate_key_instead_of_choosing() {
    let entries = vec![
        ("fan".to_owned(), Value::int(1)),
        ("fan".to_owned(), Value::int(2)),
    ];
    let error = Value::map(entries).unwrap_err();
    assert_eq!(error.path(), "entries[1].key");
    assert_eq!(error.violation(), &Violation::Duplicate);

    let empty_key = vec![(String::new(), Value::int(1))];
    let error = Value::map(empty_key).unwrap_err();
    assert_eq!(error.violation(), &Violation::Empty);
}

#[test]
fn depth_is_logical_and_bounded_where_the_schema_says() {
    // A chain of nested lists: depth 16 is the schema's bound, inclusive.
    let mut value = Value::int(0);
    for _ in 0..(limits::VALUE_MAX_DEPTH - 1) {
        value = Value::list(vec![value]).unwrap();
    }
    assert_eq!(value.depth(), limits::VALUE_MAX_DEPTH);

    let error = Value::list(vec![value]).unwrap_err();
    assert_eq!(
        error.violation(),
        &Violation::TooDeep {
            limit: limits::VALUE_MAX_DEPTH
        }
    );
}

#[test]
fn a_decoded_value_is_canonicalized_and_round_trips() {
    // Wire entries arrive unsorted; the validated form sorts them, so the
    // rebuilt wire message is the canonical representation of the same value.
    let unsorted = wire::Value {
        kind: Some(wire::value::Kind::MapValue(wire::value::Map {
            entries: vec![
                wire::value::map::Entry {
                    key: Some("outer".to_owned()),
                    value: Some(wire::Value {
                        kind: Some(wire::value::Kind::ListValue(wire::value::List {
                            values: vec![wire::Value {
                                kind: Some(wire::value::Kind::StringValue("deep".to_owned())),
                            }],
                        })),
                    }),
                },
                wire::value::map::Entry {
                    key: Some("alpha".to_owned()),
                    value: Some(wire::Value {
                        kind: Some(wire::value::Kind::DoubleValue(-0.0)),
                    }),
                },
            ],
        })),
    };

    let validated = Value::try_from(unsorted).unwrap();
    assert_eq!(validated.depth(), 3);

    let ValueKind::Map(entries) = validated.kind() else {
        panic!("a map decoded as something else");
    };
    let keys: Vec<&str> = entries.keys().map(String::as_str).collect();
    assert_eq!(keys, ["alpha", "outer"], "entries did not sort");

    let rebuilt = wire::Value::from(validated.clone());
    let redecoded = Value::try_from(rebuilt).unwrap();
    assert_eq!(redecoded, validated, "round trip changed the value");
}

#[test]
fn an_invalid_deep_in_a_tree_names_its_path() {
    let wire = wire::Value {
        kind: Some(wire::value::Kind::ListValue(wire::value::List {
            values: vec![wire::Value {
                kind: Some(wire::value::Kind::MapValue(wire::value::Map {
                    entries: vec![wire::value::map::Entry {
                        key: Some("reading".to_owned()),
                        value: Some(wire::Value {
                            kind: Some(wire::value::Kind::DoubleValue(f64::NAN)),
                        }),
                    }],
                })),
            }],
        })),
    };

    let error = Value::try_from(wire).unwrap_err();
    assert_eq!(
        error.path(),
        "values[0].map_value.entries[0].value.double_value"
    );
    assert_eq!(error.violation(), &Violation::NotFinite);
}

use crate::AcquisitionStatus;
use crate::Completeness;
use crate::Coverage;
use crate::EndpointContext;
use crate::FailureClass;
use crate::ObservationBatch;
use crate::ObservationWindow;
use crate::ObservedResource;
use crate::Origin;
use crate::Outcome;
use crate::Payload;
use crate::Reading;
use crate::Readings;
use crate::ResourceGraph;
use crate::ResourceRelation;
use crate::SignalDescriptor;
use crate::SignalKey;
use crate::Subject;
use crate::ValueRange;

fn built_subject(kind: &str, id: &str) -> Subject {
    Subject::builder()
        .kind(kind)
        .id(id)
        .build()
        .expect("a valid subject")
}

fn built_key(id: &str) -> SignalKey {
    SignalKey::builder()
        .subject(built_subject("sensor", id))
        .build()
        .expect("a valid signal key")
}

#[test]
fn an_empty_identity_is_unrepresentable() {
    // The case the vocabulary branch was for: "" is a well-formed identity
    // that every failed read would collapse into, and Subject is hashable.
    let error = Subject::builder()
        .kind("sensor")
        .id("")
        .build()
        .unwrap_err();
    assert_eq!(error.path(), "id");
    assert_eq!(error.violation(), &Violation::Empty);

    let error = Subject::builder().id("x").build().unwrap_err();
    assert_eq!(error.path(), "kind");
    assert_eq!(error.violation(), &Violation::Absent);
}

fn valid_batch() -> ObservationBatch {
    let key = built_key("CPU1Temp");
    let descriptor = SignalDescriptor::builder()
        .key(key.clone())
        .kind("temperature")
        .unit("Cel")
        .build()
        .expect("a valid descriptor");
    let sample = Reading::builder()
        .key(key)
        .value(NumericValue::double(47.5).expect("finite"))
        .build()
        .expect("a valid reading");
    let readings = Readings::builder()
        .descriptors(vec![descriptor])
        .samples(vec![sample])
        .build()
        .expect("a valid readings payload");

    ObservationBatch::builder()
        .endpoint(
            EndpointContext::builder()
                .endpoint_id("bmc-lab-07")
                .build()
                .expect("a valid endpoint"),
        )
        .origin(
            Origin::builder()
                .provider("redfish.sensor.odata")
                .request_class("sensor-read")
                .build()
                .expect("a valid origin"),
        )
        .window(
            ObservationWindow::builder()
                .start(Timestamp::new(1_785_621_243, 0).expect("a valid instant"))
                .build()
                .expect("a valid window"),
        )
        .coverage(
            Coverage::builder()
                .completeness(Completeness::Partial)
                .build()
                .expect("valid coverage"),
        )
        .payload(Payload::Readings(readings))
        .build()
        .expect("a valid batch")
}

#[test]
fn a_batch_round_trips_through_the_validated_boundary() {
    let batch = valid_batch();
    let bytes = batch.encode_to_vec();
    let decoded = ObservationBatch::decode(&bytes).expect("wire round trip");
    assert_eq!(decoded, batch);
}

#[test]
fn a_sample_without_its_descriptor_is_rejected() {
    // "Every key referenced by a sample must resolve here — a wrapper rule."
    let sample = Reading::builder()
        .key(built_key("CPU1Temp"))
        .value(NumericValue::double(1.0).expect("finite"))
        .build()
        .expect("a valid reading");
    let error = Readings::builder()
        .samples(vec![sample])
        .build()
        .unwrap_err();
    assert_eq!(error.path(), "samples[0]");
}

#[test]
fn two_descriptors_for_one_key_are_rejected() {
    // The unique_by case the annotation was written for: same key, different
    // units, and a consumer with no rule for choosing.
    let descriptor = |unit: &str| {
        SignalDescriptor::builder()
            .key(built_key("PSU1"))
            .unit(unit)
            .build()
            .expect("a valid descriptor")
    };
    let error = Readings::builder()
        .descriptors(vec![descriptor("W"), descriptor("kW.h")])
        .build()
        .unwrap_err();
    assert_eq!(error.path(), "descriptors[1]");
    assert_eq!(error.violation(), &Violation::Duplicate);
}

#[test]
fn a_failure_carries_its_class_and_a_success_does_not() {
    let base = || {
        AcquisitionStatus::builder()
            .endpoint_id("bmc-lab-07")
            .provider("redfish.sensor.odata")
            .request_class("sensor-read")
            .started_at(Timestamp::new(1_785_621_243, 0).expect("a valid instant"))
    };

    let error = base().outcome(Outcome::Failed).build().unwrap_err();
    assert_eq!(error.path(), "failure_class");

    let error = base()
        .outcome(Outcome::Succeeded)
        .failure_class(FailureClass::Timeout)
        .build()
        .unwrap_err();
    assert_eq!(error.path(), "failure_class");

    assert!(base()
        .outcome(Outcome::Failed)
        .failure_class(FailureClass::Timeout)
        .build()
        .is_ok());
}

#[test]
fn a_window_end_must_follow_its_start() {
    let start = Timestamp::new(100, 0).expect("a valid instant");

    // Equal would be a second spelling of a point, which omitting the end
    // already spells.
    let error = ObservationWindow::builder()
        .start(start)
        .end(start)
        .build()
        .unwrap_err();
    assert_eq!(error.path(), "end");

    assert!(ObservationWindow::builder()
        .start(start)
        .end(Timestamp::new(100, 1).expect("a valid instant"))
        .build()
        .is_ok());
}

#[test]
fn an_unrecognized_enum_value_survives_and_unspecified_does_not() {
    let ahead = wire::Coverage {
        completeness: Some(99),
        scope: None,
    };
    let coverage = Coverage::try_from(ahead).expect("a newer producer's value decodes");
    assert_eq!(coverage.completeness(), Completeness::Unrecognized(99));

    let rebuilt = wire::Coverage::from(coverage);
    assert_eq!(rebuilt.completeness, Some(99), "re-encoding lost the value");

    let unspecified = wire::Coverage {
        completeness: Some(0),
        scope: None,
    };
    let error = Coverage::try_from(unspecified).unwrap_err();
    assert_eq!(error.violation(), &Violation::Unspecified);
}

#[test]
fn a_complete_scoped_graph_must_hang_off_its_root() {
    let chassis = built_subject("chassis", "1U");
    let sensor = built_subject("sensor", "Inlet");
    let resource = |subject: &Subject| {
        ObservedResource::builder()
            .subject(subject.clone())
            .source_key(format!("/redfish/v1/{}", subject.id()))
            .properties_complete(true)
            .build()
            .expect("a valid resource")
    };

    let batch = |graph: ResourceGraph| {
        ObservationBatch::builder()
            .endpoint(
                EndpointContext::builder()
                    .endpoint_id("bmc-lab-07")
                    .build()
                    .expect("a valid endpoint"),
            )
            .origin(
                Origin::builder()
                    .provider("redfish.walker")
                    .request_class("subtree")
                    .build()
                    .expect("a valid origin"),
            )
            .window(
                ObservationWindow::builder()
                    .start(Timestamp::new(1_785_621_243, 0).expect("a valid instant"))
                    .build()
                    .expect("a valid window"),
            )
            .coverage(
                Coverage::builder()
                    .completeness(Completeness::Complete)
                    .scope(chassis.clone())
                    .build()
                    .expect("valid coverage"),
            )
            .payload(Payload::Resources(graph))
            .build()
    };

    // "A walk recording only each child naming its parent produces a graph
    // its own root cannot reach": no chassis->sensor edge, so the sensor is
    // unreachable and the complete claim is refused.
    let disconnected = ResourceGraph::builder()
        .resources(vec![resource(&chassis), resource(&sensor)])
        .build()
        .expect("structurally valid graph");
    let error = batch(disconnected).unwrap_err();
    assert_eq!(error.path(), "payload.resources[1]");

    let edge = ResourceRelation::builder()
        .source(chassis.clone())
        .target(sensor.clone())
        .kind("contains")
        .build()
        .expect("a valid relation");
    let connected = ResourceGraph::builder()
        .resources(vec![resource(&chassis), resource(&sensor)])
        .relations(vec![edge])
        .build()
        .expect("structurally valid graph");
    assert!(batch(connected).is_ok());
}

#[test]
fn a_range_needs_a_bound_one_arm_and_order() {
    assert!(ValueRange::builder().build().is_err(), "no bound at all");

    assert!(ValueRange::builder()
        .min(NumericValue::Int(0))
        .build()
        .is_ok());

    let mixed = ValueRange::builder()
        .min(NumericValue::Int(0))
        .max(NumericValue::double(5.0).expect("finite"))
        .build();
    assert!(mixed.is_err(), "bounds must share the signal's arm");

    let backwards = ValueRange::builder()
        .min(NumericValue::Int(5))
        .max(NumericValue::Int(1))
        .build();
    assert!(backwards.is_err(), "min must not exceed max");
}

/// Maximal wire messages: every optional field set, every payload domain
/// exercised. `TryFrom` consumes the wire message field by field and `From`
/// rebuilds it; a field one of them forgets is silent data loss, and sparse
/// round trips cannot see it. Maps carry a single entry so sorting cannot
/// reorder them, making byte equality the assertion.
// Long because it is exhaustive — one construction per payload domain with
// every field populated. Trimming it to a length limit would reopen exactly
// the blind spot it exists to close.
#[allow(clippy::too_many_lines)]
#[test]
fn every_field_survives_the_validated_round_trip() {
    let ts = |seconds: i64| wire::Timestamp {
        seconds: Some(seconds),
        nanos: Some(7),
    };
    let subject = |id: &str| wire::Subject {
        kind: Some("sensor".into()),
        scope: vec!["1U".into()],
        id: Some(id.into()),
    };
    let key = |id: &str| wire::SignalKey {
        subject: Some(subject(id)),
        facet: Some("state/counters".into()),
    };
    let map = wire::value::Map {
        entries: vec![wire::value::map::Entry {
            key: Some("serial".into()),
            value: Some(wire::Value {
                kind: Some(wire::value::Kind::StringValue("SN-1".into())),
            }),
        }],
    };
    let numeric = |value: f64| wire::NumericValue {
        kind: Some(wire::numeric_value::Kind::DoubleValue(value)),
    };

    let payloads = vec![
        wire::observation_batch::Payload::Readings(wire::Readings {
            descriptors: vec![wire::SignalDescriptor {
                key: Some(key("CPU1Temp")),
                kind: Some("temperature".into()),
                unit: Some("Cel".into()),
                range: Some(wire::ValueRange {
                    min: Some(numeric(0.0)),
                    max: Some(numeric(100.0)),
                }),
            }],
            samples: vec![wire::Reading {
                key: Some(key("CPU1Temp")),
                value: Some(numeric(47.5)),
                observed_at: Some(ts(10)),
            }],
        }),
        wire::observation_batch::Payload::Logs(wire::Logs {
            records: vec![wire::LogRecord {
                occurred_at: Some(ts(20)),
                severity: Some(3),
                message: Some("fan failed".into()),
                subject: Some(subject("Fan1")),
                entry_id: Some("sel-41".into()),
                attributes: Some(map.clone()),
            }],
        }),
        wire::observation_batch::Payload::States(wire::States {
            observations: vec![wire::StateObservation {
                subject: Some(subject("Fan1")),
                name: Some("health".into()),
                value: Some(wire::Value {
                    kind: Some(wire::value::Kind::StringValue("OK".into())),
                }),
                observed_at: Some(ts(30)),
            }],
        }),
        wire::observation_batch::Payload::Inventory(wire::Inventory {
            items: vec![wire::InventoryItem {
                subject: Some(subject("PSU1")),
                attributes: Some(map.clone()),
                source_key: Some("/redfish/v1/PSU1".into()),
            }],
        }),
        wire::observation_batch::Payload::Resources(wire::ResourceGraph {
            resources: vec![wire::ObservedResource {
                subject: Some(subject("1U")),
                source_key: Some("/redfish/v1/Chassis/1U".into()),
                source_schema: Some("#Chassis.v1_2_0.Chassis".into()),
                entity_tag: Some("W/\"e-1\"".into()),
                observed_at: Some(ts(40)),
                properties: Some(map.clone()),
                properties_complete: Some(true),
                unresolved: vec![wire::UnresolvedReference {
                    location: Some("/redfish/v1/Chassis/2U".into()),
                    property: Some("Links.ContainedBy".into()),
                }],
            }],
            relations: vec![wire::ResourceRelation {
                source: Some(subject("1U")),
                target: Some(subject("2U")),
                kind: Some("contains".into()),
            }],
        }),
    ];

    for payload in payloads {
        let maximal = wire::ObservationBatch {
            endpoint: Some(wire::EndpointContext {
                endpoint_id: Some("bmc-lab-07".into()),
                attributes: Some(map.clone()),
            }),
            origin: Some(wire::Origin {
                provider: Some("redfish".into()),
                request_class: Some("read".into()),
            }),
            window: Some(wire::ObservationWindow {
                start: Some(ts(1)),
                end: Some(ts(2)),
            }),
            coverage: Some(wire::Coverage {
                completeness: Some(wire::Completeness::Partial as i32),
                scope: Some(subject("1U")),
            }),
            payload: Some(payload),
        };

        let validated = ObservationBatch::try_from(maximal.clone()).expect("maximal batch valid");
        let rebuilt = wire::ObservationBatch::from(validated);
        assert_eq!(rebuilt, maximal, "a field was dropped on the round trip");
    }

    let status = wire::AcquisitionStatus {
        endpoint_id: Some("bmc-lab-07".into()),
        provider: Some("redfish".into()),
        request_class: Some("read".into()),
        outcome: Some(2),
        failure_class: Some(3),
        retryable: Some(true),
        started_at: Some(ts(50)),
        duration_nanos: Some(125_000),
        detail: Some("timed out".into()),
    };
    let validated = AcquisitionStatus::try_from(status.clone()).expect("maximal status valid");
    assert_eq!(
        wire::AcquisitionStatus::from(validated),
        status,
        "a status field was dropped on the round trip"
    );
}

/// A hasher that keeps the bytes: hash tests assert on the digest stream
/// itself, which is the contract, rather than on any hash function's output.
#[derive(Default)]
struct Collect(Vec<u8>);

impl std::hash::Hasher for Collect {
    fn write(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    fn finish(&self) -> u64 {
        0
    }
}

fn digest_bytes(graph: &ResourceGraph) -> Vec<u8> {
    let mut sink = Collect::default();
    graph.content_hash(&mut sink);
    sink.0
}

fn built_resource(id: &str, tag: &str) -> ObservedResource {
    ObservedResource::builder()
        .subject(built_subject("chassis", id))
        .source_key(format!("/redfish/v1/Chassis/{id}"))
        .entity_tag(tag)
        .properties_complete(true)
        .build()
        .expect("a valid resource")
}

#[test]
fn canonicalization_makes_input_order_vanish() {
    let forward = ResourceGraph::builder()
        .resources(vec![built_resource("A", "e1"), built_resource("B", "e2")])
        .build()
        .expect("a valid graph");
    let backward = ResourceGraph::builder()
        .resources(vec![built_resource("B", "e2"), built_resource("A", "e1")])
        .build()
        .expect("a valid graph");

    // Same content, one representation: equal values, equal wire bytes,
    // equal hash streams — regardless of arrival order.
    assert_eq!(forward, backward);
    assert_eq!(
        wire::ResourceGraph::from(forward.clone()).encode_to_vec(),
        wire::ResourceGraph::from(backward.clone()).encode_to_vec(),
        "canonical wire bytes depend on input order"
    );
    assert_eq!(digest_bytes(&forward), digest_bytes(&backward));

    // And the order a consumer sees is the canonical one.
    let ids: Vec<&str> = forward
        .resources()
        .iter()
        .map(|resource| resource.subject().id())
        .collect();
    assert_eq!(ids, ["A", "B"]);
}

#[test]
fn collection_metadata_breaks_ties_but_never_moves_the_hash() {
    // Two graphs identical in everything hashing sees, different in entity
    // tags: the reason the annotation exists is that a re-poll must not read
    // as a device change.
    let first = ResourceGraph::builder()
        .resources(vec![built_resource("A", "before")])
        .build()
        .expect("a valid graph");
    let second = ResourceGraph::builder()
        .resources(vec![built_resource("A", "after")])
        .build()
        .expect("a valid graph");

    assert_eq!(digest_bytes(&first), digest_bytes(&second));
    assert_ne!(first, second, "the tags are still content of the value");

    // A hash-visible difference moves the stream.
    let third = ResourceGraph::builder()
        .resources(vec![ObservedResource::builder()
            .subject(built_subject("chassis", "A"))
            .source_key("/redfish/v1/Chassis/other")
            .entity_tag("before")
            .properties_complete(true)
            .build()
            .expect("a valid resource")])
        .build()
        .expect("a valid graph");
    assert_ne!(digest_bytes(&first), digest_bytes(&third));

    // Ties on every hash-visible field still order deterministically, by
    // the metadata tiebreak. Pinned at the comparator: two such elements can
    // never share a collection — equal subjects are duplicates by design, and
    // that is the uniqueness test above — but the total order must still
    // decide them, or canonical bytes would depend on the sort's whims.
    {
        use crate::canonical::Canonical as _;
        let earlier = built_resource("A", "a-earlier");
        let later = built_resource("A", "z-later");
        assert_eq!(
            earlier.canonical_cmp(&later),
            std::cmp::Ordering::Less,
            "metadata did not break the tie"
        );
        let mut left = Collect::default();
        let mut right = Collect::default();
        crate::canonical::Digest::digest(&earlier, &mut left);
        crate::canonical::Digest::digest(&later, &mut right);
        assert_eq!(left.0, right.0, "the tiebreaker leaked into the digest");
    }
}

#[test]
fn the_digest_stream_is_injective_where_concatenation_would_lie() {
    use crate::canonical::Digest as _;

    let stream = |kind: &str, id: &str| {
        let mut sink = Collect::default();
        built_subject(kind, id).content_hash(&mut sink);
        sink.0
    };
    // Without length prefixes these two would concatenate identically.
    assert_ne!(stream("ab", "c"), stream("a", "bc"));

    // An integer and a double holding the same number are different content:
    // arm selection is fixed by the source's declared type.
    let arm = |value: Value| {
        let mut sink = Collect::default();
        value.digest(&mut sink);
        sink.0
    };
    assert_ne!(arm(Value::int(5)), arm(Value::uint(5)));
    assert_ne!(arm(Value::int(5)), arm(Value::double(5.0).expect("finite")));
}

#[test]
fn duplicates_are_caught_even_when_they_arrive_far_apart() {
    // The adjacent scan only works because canonicalization sorted first;
    // this pins that ordering, with the duplicates separated on arrival.
    let error = ResourceGraph::builder()
        .resources(vec![
            built_resource("A", "e1"),
            built_resource("B", "e2"),
            built_resource("A", "e3"),
        ])
        .build()
        .unwrap_err();
    assert_eq!(error.violation(), &Violation::Duplicate);
}

#[test]
fn public_ord_on_identities_matches_the_canonical_order() {
    use crate::canonical::Canonical as _;

    // `rules::readings` binary-searches canonically sorted descriptors using
    // the public `Ord`, which is sound only while the two orders agree. They
    // agree today because Subject and SignalKey have no metadata fields and
    // declare their fields in number order; if this test fails, a schema
    // change broke that coincidence and the rules must switch to the
    // canonical comparator.
    // Scoped and unscoped subjects both, because the coincidence must cover
    // every field: `scope` is the one Vec in the pair, and `facet` the one
    // Option, and either diverging between the two orders would silently
    // corrupt the rules' binary searches.
    let scoped = Subject::builder()
        .kind("sensor")
        .scope(vec!["1U".into(), "PSU1".into()])
        .id("A")
        .build()
        .expect("a valid subject");
    let subjects = [
        built_subject("chassis", "A"),
        built_subject("sensor", "B"),
        built_subject("chassis", "B"),
        scoped,
    ];
    for left in &subjects {
        for right in &subjects {
            assert_eq!(left.cmp(right), left.canonical_cmp(right));
        }
    }

    let faceted = SignalKey::builder()
        .subject(built_subject("sensor", "A"))
        .facet("state/counters")
        .build()
        .expect("a valid key");
    let keys = [built_key("A"), built_key("B"), faceted];
    for left in &keys {
        for right in &keys {
            assert_eq!(left.cmp(right), left.canonical_cmp(right));
        }
    }
}
