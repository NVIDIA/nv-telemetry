// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Pins the property the `max_depth` bound exists to guarantee: a `Value` at
//! the declared depth, nested inside the deepest real batch path, survives a
//! decode.
//!
//! The bound was derived by measurement, and the measured ceiling differs by
//! a level between prost's generated decoder and prost-reflect's dynamic one,
//! so this test uses the dynamic decoder — the tighter of the two — and the
//! deepest enclosing path the contract has. If a future message nests the
//! recursion one level deeper, or a dependency changes its recursion
//! accounting, this fails before a consumer's decode does.

use nv_telemetry_codegen::options::Vocabulary;
use prost::Message as _;
use prost_reflect::DescriptorPool;
use prost_reflect::DynamicMessage;
use prost_reflect::Value as V;

fn nested_value(pool: &DescriptorPool, depth: u32) -> DynamicMessage {
    let value_d = pool.get_message_by_name("nv.telemetry.v1.Value").unwrap();
    let map_d = pool
        .get_message_by_name("nv.telemetry.v1.Value.Map")
        .unwrap();
    let entry_d = pool
        .get_message_by_name("nv.telemetry.v1.Value.Map.Entry")
        .unwrap();

    let mut current = DynamicMessage::new(value_d.clone());
    current.set_field_by_name("bool_value", V::Bool(true));

    // Map nesting is the expensive variant: three message levels per logical
    // level, against two for lists.
    for _ in 0..depth {
        let mut entry = DynamicMessage::new(entry_d.clone());
        entry.set_field_by_name("key", V::String("k".into()));
        entry.set_field_by_name("value", V::Message(current));

        let mut map = DynamicMessage::new(map_d.clone());
        map.set_field_by_name("entries", V::List(vec![V::Message(entry)]));

        let mut value = DynamicMessage::new(value_d.clone());
        value.set_field_by_name("map_value", V::Message(map));
        current = value;
    }
    current
}

fn batch_with_value_at(pool: &DescriptorPool, depth: u32) -> Vec<u8> {
    let batch_d = pool
        .get_message_by_name("nv.telemetry.v1.ObservationBatch")
        .unwrap();
    let graph_d = pool
        .get_message_by_name("nv.telemetry.v1.ResourceGraph")
        .unwrap();
    let resource_d = pool
        .get_message_by_name("nv.telemetry.v1.ObservedResource")
        .unwrap();
    let map_d = pool
        .get_message_by_name("nv.telemetry.v1.Value.Map")
        .unwrap();
    let entry_d = pool
        .get_message_by_name("nv.telemetry.v1.Value.Map.Entry")
        .unwrap();

    let mut entry = DynamicMessage::new(entry_d);
    entry.set_field_by_name("key", V::String("p".into()));
    entry.set_field_by_name("value", V::Message(nested_value(pool, depth)));

    let mut properties = DynamicMessage::new(map_d);
    properties.set_field_by_name("entries", V::List(vec![V::Message(entry)]));

    let mut resource = DynamicMessage::new(resource_d);
    resource.set_field_by_name("properties", V::Message(properties));

    let mut graph = DynamicMessage::new(graph_d);
    graph.set_field_by_name("resources", V::List(vec![V::Message(resource)]));

    let mut batch = DynamicMessage::new(batch_d);
    batch.set_field_by_name("resources", V::Message(graph));
    batch.encode_to_vec()
}

#[test]
fn a_value_at_max_depth_in_the_deepest_batch_path_decodes() {
    let pool = nv_telemetry_codegen::pool().expect("shipped schema decodes");
    let vocabulary = Vocabulary::resolve(&pool).expect("shipped schema defines the vocabulary");

    let value = pool.get_message_by_name("nv.telemetry.v1.Value").unwrap();
    let declared = vocabulary
        .message_invariant(&value)
        .and_then(|invariant| invariant.max_depth)
        .expect("Value declares max_depth");

    let batch_d = pool
        .get_message_by_name("nv.telemetry.v1.ObservationBatch")
        .unwrap();

    let bytes = batch_with_value_at(&pool, declared);
    assert!(
        DynamicMessage::decode(batch_d.clone(), bytes.as_slice()).is_ok(),
        "a Value at the declared max_depth ({declared}) failed to decode inside a graph \
         batch; validation would accept a batch no consumer can read"
    );

    // Well past every measured ceiling, so the test demonstrably has teeth.
    // If this starts passing, the runtime raised its recursion limit and the
    // bound's margin should be re-derived.
    let bytes = batch_with_value_at(&pool, declared * 3);
    assert!(
        DynamicMessage::decode(batch_d, bytes.as_slice()).is_err(),
        "a Value at three times max_depth decoded; the recursion limit this bound was \
         derived against has changed"
    );
}
