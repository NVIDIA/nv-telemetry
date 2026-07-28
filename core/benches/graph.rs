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

//! Instruction-count benchmarks for the observed resource graph (gungraun /
//! Valgrind Callgrind).
//!
//! A graph snapshot is assembled once per collection and then validated,
//! traversed, and often encoded, so those four are what these measure.
//! Assembly is the interesting one: it sorts resources and relations and
//! makes four passes looking for duplicates, all of which grow with the
//! graph. Resources are built in setup and passed in. Valgrind is
//! unix-only, so the whole benchmark is `cfg(unix)`.

// The gungraun attribute macros re-emit each setup expression with fully
// qualified paths, which the workspace's `unused_qualifications` lint counts
// against the call site. Nothing here is written qualified by hand.
#![allow(unused_qualifications)]

#[cfg(unix)]
mod unix {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hash;
    use std::hash::Hasher;
    use std::hint::black_box;
    use std::sync::Arc;

    use gungraun::library_benchmark;
    use nv_telemetry_core::Attributes;
    use nv_telemetry_core::BatchError;
    use nv_telemetry_core::Coverage;
    use nv_telemetry_core::EndpointContext;
    use nv_telemetry_core::ObservationBatch;
    use nv_telemetry_core::ObservationWindow;
    use nv_telemetry_core::ObservedResource;
    use nv_telemetry_core::Origin;
    use nv_telemetry_core::Payload;
    use nv_telemetry_core::Property;
    use nv_telemetry_core::PropertyMap;
    use nv_telemetry_core::PropertyValue;
    use nv_telemetry_core::ResourceGraph;
    use nv_telemetry_core::ResourceReference;
    use nv_telemetry_core::ResourceRelation;
    use nv_telemetry_core::Subject;
    use nv_telemetry_core::Timestamp;

    /// A chassis subtree: one root and the systems, processors, and sensors
    /// beneath it.
    const RESOURCES: usize = 128;

    /// Size for the rejection path, where the cost of getting it wrong grows
    /// with the square of the graph.
    const DETACHED_RESOURCES: usize = 512;

    fn timestamp() -> Timestamp {
        Timestamp::new(1_700_000_000, 0).expect("valid timestamp")
    }

    fn subject(index: usize) -> Subject {
        Subject::new("resource", format!("node-{index}"))
    }

    /// One resource's properties in the order a decoder yields them, which
    /// is not sorted, with the nesting a real representation carries.
    fn property_list(index: usize) -> Vec<Property> {
        let firmware = PropertyMap::new(vec![
            Property::new("version", format!("1.{index}.0")),
            Property::new("updateable", true),
            Property::new("checksum", PropertyValue::Bytes(Arc::from([0xau8; 32]))),
        ])
        .expect("unique property names");

        vec![
            Property::new("power_state", "on"),
            Property::new("firmware", firmware),
            Property::new("name", format!("Node {index}")),
            Property::new("health", "OK"),
            Property::new(
                "addresses",
                PropertyValue::Array(
                    vec![
                        PropertyValue::from("192.0.2.10"),
                        PropertyValue::from("2001:db8::10"),
                    ]
                    .into_boxed_slice(),
                ),
            ),
            Property::new("index", index as u64),
            Property::new("last_reset", PropertyValue::Timestamp(timestamp())),
            Property::new("oem_reserved", PropertyValue::Null),
            Property::new(
                "manager",
                ResourceReference::new("/redfish/v1/Managers/BMC")
                    .with_subject(Subject::new("manager", "bmc")),
            ),
            Property::new("serial", format!("SN-{index:06}")),
        ]
    }

    fn properties(index: usize) -> PropertyMap {
        PropertyMap::new(property_list(index)).expect("unique property names")
    }

    /// A containment tree rooted at node 0, so every resource is reachable
    /// from the root by outgoing edges and the traversal has real depth.
    fn parts(count: usize) -> (Vec<ObservedResource>, Vec<ResourceRelation>) {
        let resources = (0..count)
            .map(|index| {
                ObservedResource::complete(
                    subject(index),
                    format!("/redfish/v1/Chassis/1/Nodes/{index}"),
                    properties(index),
                )
                .with_schema("#Chassis.v1_25_0.Chassis")
                .with_version(format!("W/\"etag-{index}\""))
                .with_observed_at(timestamp())
            })
            .collect();
        let relations = (1..count)
            .map(|index| {
                ResourceRelation::new(subject((index - 1) / 2), "contains", subject(index))
            })
            .collect();
        (resources, relations)
    }

    fn graph(count: usize) -> ResourceGraph {
        let (resources, relations) = parts(count);
        ResourceGraph::new(resources, relations).expect("valid graph")
    }

    fn batch_input(count: usize, coverage: Coverage) -> (ResourceGraph, Coverage) {
        (graph(count), coverage)
    }

    fn batch(count: usize) -> ObservationBatch {
        assemble(graph(count), Coverage::complete_endpoint())
    }

    /// A graph holding one resource the root cannot reach, which is the shape
    /// scope validation rejects.
    ///
    /// Larger than the other cases on purpose. The rejection path is reachable
    /// from `Deserialize`, so an endpoint decides how often it runs; at this
    /// size a scan-per-resource implementation costs an order of magnitude
    /// more than a traversal rather than a few percent, so losing the
    /// distinction shows up as a jump here.
    fn detached(count: usize) -> (ResourceGraph, Coverage) {
        let (resources, mut relations) = parts(count);
        let orphan = subject(count - 1);
        relations.retain(|relation| relation.target != orphan);
        (
            ResourceGraph::new(resources, relations).expect("valid graph"),
            Coverage::complete_subject(subject(0)),
        )
    }

    /// A batch whose collections have already been asked for their digest,
    /// which is the state anything holding a previous snapshot is in.
    fn hashed(count: usize) -> ObservationBatch {
        let batch = batch(count);
        digest(&batch);
        batch
    }

    fn digest(batch: &ObservationBatch) -> u64 {
        let mut hasher = DefaultHasher::new();
        batch.hash(&mut hasher);
        hasher.finish()
    }

    fn try_assemble(
        graph: ResourceGraph,
        coverage: Coverage,
    ) -> Result<ObservationBatch, BatchError> {
        ObservationBatch::new(
            Arc::new(EndpointContext::new("bmc-1", Attributes::empty())),
            Origin::new("redfish-walk", "chassis-subtree"),
            ObservationWindow::point(timestamp()),
            coverage,
            Payload::Resources(graph),
        )
    }

    fn assemble(graph: ResourceGraph, coverage: Coverage) -> ObservationBatch {
        try_assemble(graph, coverage).expect("valid batch")
    }

    fn encoded(count: usize) -> String {
        serde_json::to_string(&batch(count)).expect("serialize graph batch")
    }

    // Sorting one resource's properties, rejecting duplicates, and walking
    // each value for nesting depth.
    #[library_benchmark]
    #[bench::properties_10(property_list(0))]
    fn property_map_new(properties: Vec<Property>) -> PropertyMap {
        black_box(PropertyMap::new(properties)).expect("unique property names")
    }

    // Graph assembly: two sorts plus the duplicate-subject, duplicate-source,
    // duplicate-edge, and unknown-source passes.
    #[library_benchmark]
    #[bench::resources_128(parts(RESOURCES))]
    fn graph_new(
        (resources, relations): (Vec<ObservedResource>, Vec<ResourceRelation>),
    ) -> ResourceGraph {
        black_box(ResourceGraph::new(resources, relations)).expect("valid graph")
    }

    // Batch assembly over a graph. A subject scope walks the tree from the
    // root to prove every resource is reachable; an endpoint scope does not.
    #[library_benchmark]
    #[bench::endpoint_scope(batch_input(RESOURCES, Coverage::complete_endpoint()))]
    #[bench::subject_scope(batch_input(RESOURCES, Coverage::complete_subject(subject(0))))]
    fn validate_scope((graph, coverage): (ResourceGraph, Coverage)) -> ObservationBatch {
        black_box(assemble(graph, coverage))
    }

    // Rejecting a graph that does not match its declared scope, which costs
    // the traversal plus naming the resource that fell outside it.
    #[library_benchmark]
    #[bench::detached_resource(detached(DETACHED_RESOURCES))]
    fn reject_scope((graph, coverage): (ResourceGraph, Coverage)) -> BatchError {
        black_box(
            try_assemble(graph, coverage).expect_err("an unreachable resource is out of scope"),
        )
    }

    // Content-addressing a snapshot. Each collection digests its entries once
    // and caches it, so `warm` is what a consumer holding a previous snapshot
    // pays and `cold` is what building the digests costs the first time.
    #[library_benchmark]
    #[bench::cold(batch(RESOURCES))]
    #[bench::warm(hashed(RESOURCES))]
    fn hash_graph(batch: ObservationBatch) -> ObservationBatch {
        black_box(digest(&batch));
        batch
    }

    // Binary search for one subject's outgoing edges.
    #[library_benchmark]
    #[bench::resources_128((graph(RESOURCES), subject(RESOURCES / 2)))]
    fn relations_from((graph, subject): (ResourceGraph, Subject)) -> ResourceGraph {
        black_box(graph.relations_from(&subject));
        graph
    }

    #[library_benchmark]
    #[bench::resources_128(batch(RESOURCES))]
    fn encode(batch: ObservationBatch) -> (ObservationBatch, String) {
        let json = serde_json::to_string(&batch).expect("serialize graph batch");
        (batch, black_box(json))
    }

    // Decoding revalidates the graph and its scope, which is not optional
    // on the wire path and so is inside the measurement.
    #[library_benchmark]
    #[bench::resources_128(encoded(RESOURCES))]
    fn decode(json: String) -> (String, ObservationBatch) {
        let batch: ObservationBatch =
            serde_json::from_str(black_box(&json)).expect("deserialize graph batch");
        (json, black_box(batch))
    }
}

#[cfg(unix)]
use unix::decode;
#[cfg(unix)]
use unix::encode;
#[cfg(unix)]
use unix::graph_new;
#[cfg(unix)]
use unix::hash_graph;
#[cfg(unix)]
use unix::property_map_new;
#[cfg(unix)]
use unix::reject_scope;
#[cfg(unix)]
use unix::relations_from;
#[cfg(unix)]
use unix::validate_scope;

#[cfg(unix)]
gungraun::library_benchmark_group!(
    name = assembly;
    benchmarks = property_map_new, graph_new, validate_scope, reject_scope
);

#[cfg(unix)]
gungraun::library_benchmark_group!(
    name = access;
    benchmarks = relations_from, hash_graph, encode, decode
);

#[cfg(unix)]
gungraun::main!(library_benchmark_groups = assembly, access);

#[cfg(not(unix))]
fn main() {}
