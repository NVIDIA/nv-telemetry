// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Property-based checks over the model's central claims, on inputs no
//! fixture thought of.
//!
//! The example tests pin every arm and every field; what they cannot pin is
//! the space between examples — the i64 whose zigzag crosses a varint width
//! boundary, the map whose keys collide with a list's count prefix, the tree
//! shape nobody wrote down. Each property here is one of the claims the
//! module docs argue by construction, checked by generation instead:
//!
//! - the direct encoder produces prost's bytes, and `emitted_len` is the
//!   length it produces, for arbitrary valid values;
//! - the canonical order is a total order consistent with equality;
//! - input order vanishes: a graph built from any permutation of its
//!   collections has one digest stream and one wire form.

use std::cmp::Ordering;

use proptest::prelude::*;
use prost::Message as _;

use super::canonical::digest_bytes;
use crate::canonical::Canonical as _;
use crate::encode::Emit as _;
use crate::generated::wire;
use crate::NumericValue;
use crate::ObservedResource;
use crate::ResourceGraph;
use crate::Subject;
use crate::Timestamp;
use crate::Value;

fn timestamp() -> impl Strategy<Value = Timestamp> {
    (any::<i64>(), 0..1_000_000_000u32)
        .prop_map(|(seconds, nanos)| Timestamp::new(seconds, nanos).expect("nanos in bounds"))
}

fn finite() -> impl Strategy<Value = f64> {
    any::<f64>().prop_filter("finite doubles only", |value| value.is_finite())
}

fn numeric_value() -> impl Strategy<Value = NumericValue> {
    prop_oneof![
        finite().prop_map(|value| NumericValue::double(value).expect("finite")),
        any::<i64>().prop_map(NumericValue::Int),
        any::<u64>().prop_map(NumericValue::Uint),
    ]
}

/// A map key within the schema's bounds: non-empty, short.
fn map_key() -> impl Strategy<Value = String> {
    "[a-z_]{1,12}"
}

/// An arbitrary valid value: every scalar arm as a leaf, lists and maps as
/// containers, nested well inside the schema's depth bound.
fn value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::null()),
        any::<bool>().prop_map(Value::bool),
        any::<i64>().prop_map(Value::int),
        any::<u64>().prop_map(Value::uint),
        finite().prop_map(|value| Value::double(value).expect("finite")),
        "[ -~]{0,24}".prop_map(|text| Value::string(text).expect("within bounds")),
        proptest::collection::vec(any::<u8>(), 0..24)
            .prop_map(|bytes| Value::bytes(bytes).expect("within bounds")),
        timestamp().prop_map(Value::timestamp),
    ];
    leaf.prop_recursive(4, 24, 4, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4)
                .prop_map(|values| Value::list(values).expect("within bounds")),
            proptest::collection::btree_map(map_key(), inner, 0..4)
                .prop_map(|entries| Value::map(entries).expect("within bounds")),
        ]
    })
}

/// Distinct resource ids in canonical order, and the same ids in an arbitrary
/// permutation — the two input orders whose outputs must not differ.
fn permuted_ids() -> impl Strategy<Value = (Vec<String>, Vec<String>)> {
    proptest::collection::btree_set("[a-z]{1,6}", 1..6).prop_flat_map(|ids| {
        let sorted: Vec<String> = ids.into_iter().collect();
        let shuffled = Just(sorted.clone()).prop_shuffle();
        (Just(sorted), shuffled)
    })
}

fn resource(id: &str) -> ObservedResource {
    ObservedResource::builder()
        .subject(
            Subject::builder()
                .kind("chassis")
                .id(id)
                .build()
                .expect("a valid subject"),
        )
        .source_key(format!("/redfish/v1/Chassis/{id}"))
        .properties([(format!("name_{id}"), Value::string(id).expect("short"))].into())
        .properties_complete(true)
        .build()
        .expect("a valid resource")
}

fn graph(ids: &[String]) -> ResourceGraph {
    ResourceGraph::builder()
        .resources(ids.iter().map(|id| resource(id)).collect())
        .build()
        .expect("a valid graph")
}

proptest! {
    #[test]
    fn the_direct_encoder_matches_prost_for_any_value(value in value()) {
        let mut direct = Vec::new();
        value.emit(&mut direct);
        prop_assert_eq!(direct.len(), value.emitted_len(), "emitted_len lied");
        prop_assert_eq!(
            direct,
            wire::Value::from(value).encode_to_vec(),
            "the direct encoder diverged from prost"
        );
    }

    #[test]
    fn the_direct_encoder_matches_prost_for_any_numeric(numeric in numeric_value()) {
        let mut direct = Vec::new();
        numeric.emit(&mut direct);
        prop_assert_eq!(direct.len(), numeric.emitted_len(), "emitted_len lied");
        prop_assert_eq!(direct, wire::NumericValue::from(numeric).encode_to_vec());
    }

    #[test]
    fn the_direct_encoder_matches_prost_for_any_instant(instant in timestamp()) {
        let mut direct = Vec::new();
        instant.emit(&mut direct);
        prop_assert_eq!(direct.len(), instant.emitted_len(), "emitted_len lied");
        prop_assert_eq!(direct, wire::Timestamp::from(instant).encode_to_vec());
    }

    #[test]
    fn canonical_order_agrees_with_equality_and_itself(a in value(), b in value()) {
        let forward = a.canonical_cmp(&b);
        prop_assert_eq!(forward, b.canonical_cmp(&a).reverse(), "not antisymmetric");
        prop_assert_eq!(forward == Ordering::Equal, a == b, "order and equality disagree");
    }

    #[test]
    fn canonical_order_is_transitive(mut values in proptest::collection::vec(value(), 3)) {
        values.sort_by(Value::canonical_cmp);
        for (position, left) in values.iter().enumerate() {
            for right in &values[position + 1..] {
                prop_assert_ne!(
                    left.canonical_cmp(right),
                    Ordering::Greater,
                    "sorted output has an inversion, so the order is not transitive"
                );
            }
        }
    }

    #[test]
    fn input_order_vanishes_from_a_graph(orders in permuted_ids()) {
        let (sorted, shuffled) = orders;
        let canonical = graph(&sorted);
        let permuted = graph(&shuffled);
        prop_assert_eq!(
            digest_bytes(&canonical),
            digest_bytes(&permuted),
            "the digest stream depends on input order"
        );
        prop_assert_eq!(
            wire::ResourceGraph::from(canonical).encode_to_vec(),
            wire::ResourceGraph::from(permuted).encode_to_vec(),
            "the wire form depends on input order"
        );
    }
}
