// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The cross-field rules' non-trivial paths: the binary searches whose
//! correctness the rules' own comments rest on canonical ordering.
//!
//! The `readings` and `resource_graph` rules resolve each sample's key or
//! each relation's source with a `binary_search` over the canonically sorted
//! descriptor or resource list — "the key is the canonical order's most
//! significant field". An empty list pins nothing about that search, so these
//! tests drive the search itself: a middle hit, and an absent key that sorts
//! *among* the present ones and must still be concluded absent rather than
//! landing on a neighbour.

use super::boundary::built_key;
use super::boundary::built_subject;
use crate::NumericValue;
use crate::ObservedResource;
use crate::Reading;
use crate::Readings;
use crate::ResourceGraph;
use crate::ResourceRelation;
use crate::SignalDescriptor;
use crate::Subject;
use crate::Violation;

fn built_resource(subject: &Subject) -> ObservedResource {
    ObservedResource::builder()
        .subject(subject.clone())
        .source_key(format!("/redfish/v1/{}", subject.id()))
        .properties_complete(true)
        .build()
        .expect("a valid resource")
}

#[test]
fn a_sample_key_is_found_in_the_middle_of_the_descriptors() {
    let descriptor = |id: &str| {
        SignalDescriptor::builder()
            .key(built_key(id))
            .kind("temperature")
            .unit("Cel")
            .build()
            .expect("a valid descriptor")
    };
    let sample = |id: &str| {
        Reading::builder()
            .key(built_key(id))
            .value(NumericValue::double(47.5).expect("finite"))
            .build()
            .expect("a valid reading")
    };

    // "c" sorts between "a" and "e"; the search must find it, not miss it
    // because it is not the first or last descriptor.
    let readings = Readings::builder()
        .descriptors(vec![descriptor("a"), descriptor("c"), descriptor("e")])
        .samples(vec![sample("c")])
        .build();
    assert!(
        readings.is_ok(),
        "a middle sample key must resolve: {readings:?}"
    );
}

#[test]
fn a_sample_key_absent_from_the_descriptors_is_refused_where_it_sorts() {
    let descriptor = |id: &str| {
        SignalDescriptor::builder()
            .key(built_key(id))
            .kind("temperature")
            .unit("Cel")
            .build()
            .expect("a valid descriptor")
    };
    let sample = |id: &str| {
        Reading::builder()
            .key(built_key(id))
            .value(NumericValue::double(47.5).expect("finite"))
            .build()
            .expect("a valid reading")
    };

    // "b" and "d" sort among the descriptor keys "a", "c", "e" — the search
    // must conclude absent for both, refusing the first sample its canonical
    // slot puts in its way.
    let readings = Readings::builder()
        .descriptors(vec![descriptor("a"), descriptor("c"), descriptor("e")])
        .samples(vec![sample("b"), sample("d")])
        .build();
    let error = readings.unwrap_err();
    assert_eq!(error.path(), "samples[0]");
    assert_eq!(
        error.violation(),
        &Violation::Rule("every sample's key resolves to a descriptor in this batch")
    );
}

#[test]
fn a_relation_source_is_found_in_the_middle_of_the_resources() {
    let subject = |id: &str| built_subject("chassis", id);
    let relation = |source: &str, target: &str| {
        ResourceRelation::builder()
            .source(subject(source))
            .target(subject(target))
            .kind("contains")
            .build()
            .expect("a valid relation")
    };

    // Sources at every position — first, middle, last — all resolve.
    let graph = ResourceGraph::builder()
        .resources(vec![
            built_resource(&subject("1")),
            built_resource(&subject("2")),
            built_resource(&subject("3")),
        ])
        .relations(vec![
            relation("2", "1"),
            relation("3", "2"),
            relation("1", "2"),
        ])
        .build();
    assert!(graph.is_ok(), "present sources must resolve: {graph:?}");
}

#[test]
fn a_relation_source_absent_from_the_resources_is_refused_where_it_sorts() {
    let subject = |id: &str| built_subject("chassis", id);

    // "2.5" sorts between the present subjects "2" and "3"; the search must
    // conclude absent rather than land on either neighbour.
    let graph = ResourceGraph::builder()
        .resources(vec![
            built_resource(&subject("1")),
            built_resource(&subject("2")),
            built_resource(&subject("3")),
        ])
        .relations(vec![ResourceRelation::builder()
            .source(subject("2.5"))
            .target(subject("1"))
            .kind("contains")
            .build()
            .expect("a valid relation")])
        .build();
    let error = graph.unwrap_err();
    assert_eq!(error.path(), "relations[0]");
    assert_eq!(
        error.violation(),
        &Violation::Rule("a relation's source names a resource present in the graph")
    );
}
