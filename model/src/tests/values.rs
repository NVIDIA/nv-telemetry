// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The hand-written value vocabulary: bounds, one-representation rules, and
//! the sorted, duplicate-free map.

use crate::generated::limits;
use crate::generated::wire;
use crate::Finite;
use crate::NumericValue;
use crate::Timestamp;
use crate::Value;
use crate::ValueKind;
use crate::Violation;

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
