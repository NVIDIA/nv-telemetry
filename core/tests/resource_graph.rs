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

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use nv_telemetry_core::Attributes;
use nv_telemetry_core::BatchError;
use nv_telemetry_core::Coverage;
use nv_telemetry_core::DurationValue;
use nv_telemetry_core::EndpointContext;
use nv_telemetry_core::GraphLimits;
use nv_telemetry_core::ObservationBatch;
use nv_telemetry_core::ObservationScope;
use nv_telemetry_core::ObservationWindow;
use nv_telemetry_core::ObservedResource;
use nv_telemetry_core::Origin;
use nv_telemetry_core::Payload;
use nv_telemetry_core::Property;
use nv_telemetry_core::PropertyMap;
use nv_telemetry_core::PropertyMapError;
use nv_telemetry_core::PropertyValue;
use nv_telemetry_core::ResourceCompleteness;
use nv_telemetry_core::ResourceGraph;
use nv_telemetry_core::ResourceGraphBuilder;
use nv_telemetry_core::ResourceGraphError;
use nv_telemetry_core::ResourceReference;
use nv_telemetry_core::ResourceRelation;
use nv_telemetry_core::Subject;
use nv_telemetry_core::Timestamp;

fn timestamp() -> Timestamp {
    Timestamp::new(1_721_000_000, 0).expect("valid timestamp")
}

fn endpoint() -> Arc<EndpointContext> {
    Arc::new(EndpointContext::new("bmc-1", Attributes::empty()))
}

fn host_subject() -> Subject {
    Subject::new("computer_system", "host-1")
}

fn dpu_subject() -> Subject {
    Subject::new("dpu", "dpu-0")
}

fn properties(values: Vec<Property>) -> PropertyMap {
    PropertyMap::new(values).expect("unique property names")
}

fn host_resource() -> ObservedResource {
    let firmware = properties(vec![
        Property::new("bios", "2.4.1"),
        Property::new("bmc", "1.8.0"),
    ]);
    let network = properties(vec![
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
        Property::new("connected", true),
    ]);
    ObservedResource::complete(
        host_subject(),
        "/redfish/v1/Systems/host-1",
        properties(vec![
            Property::new("bios_settings_hash", "sha256:abc"),
            Property::new("firmware", firmware),
            Property::new("network", network),
            Property::new("power_state", "on"),
            Property::new("nullable_oem_value", PropertyValue::Null),
            Property::new(
                "uptime",
                DurationValue::new(86_400, 250_000_000).expect("valid duration"),
            ),
        ]),
    )
    .with_schema("#ComputerSystem.v1_20_0.ComputerSystem")
    .with_version("W/\"etag-7\"")
    .with_observed_at(timestamp())
}

fn dpu_resource() -> ObservedResource {
    ObservedResource::complete(
        dpu_subject(),
        "/redfish/v1/Systems/host-1/Processors/DPU0",
        properties(vec![
            Property::new("agent_version", "3.2.1"),
            Property::new(
                "manager",
                ResourceReference::new("/redfish/v1/Managers/BMC")
                    .with_subject(Subject::new("manager", "bmc")),
            ),
            Property::new("mode", "nic"),
        ]),
    )
}

#[test]
fn property_maps_preserve_nested_values_and_nulls() {
    let map = host_resource().properties.clone();

    assert_eq!(map.get("power_state"), Some(&PropertyValue::from("on")));
    assert_eq!(map.get("nullable_oem_value"), Some(&PropertyValue::Null));
    assert!(matches!(
        map.get("uptime"),
        Some(PropertyValue::Duration(uptime)) if uptime.seconds() == 86_400
    ));
    assert_eq!(map.as_slice()[0].name.as_str(), "bios_settings_hash");
    assert!(matches!(
        map.get("firmware"),
        Some(PropertyValue::Object(_))
    ));
    assert!(matches!(map.get("network"), Some(PropertyValue::Object(_))));
}

#[test]
fn property_maps_reject_duplicate_names() {
    let duplicate = PropertyMap::new(vec![
        Property::new("state", "enabled"),
        Property::new("state", "disabled"),
    ]);

    assert!(matches!(
        duplicate,
        Err(PropertyMapError::DuplicateName(name)) if name.as_str() == "state"
    ));
}

#[test]
fn property_nesting_is_bounded() {
    assert!(nest_arrays(PropertyMap::MAX_DEPTH - 1).is_ok());
    assert!(nest_arrays(PropertyMap::MAX_DEPTH).is_ok());
    assert!(matches!(
        nest_arrays(PropertyMap::MAX_DEPTH + 1),
        Err(PropertyMapError::DepthExceeded { name, limit })
            if name.as_str() == "oem" && limit == PropertyMap::MAX_DEPTH
    ));

    // Objects count against the same budget, one level per map.
    assert!(nest_objects(PropertyMap::MAX_DEPTH).is_ok());
    assert!(matches!(
        nest_objects(PropertyMap::MAX_DEPTH + 1),
        Err(PropertyMapError::DepthExceeded { name, .. }) if name.as_str() == "oem"
    ));

    // Rejecting is not enough on its own: releasing a value this deep through
    // the recursive drop glue would abort the process.
    assert!(nest_arrays(500_000).is_err());

    // Only arrays nest without bound, since an object holds a map that already
    // passed the check. A rejected value has to release safely either way.
    let mut mixed = PropertyValue::Object(
        PropertyMap::new(vec![Property::new("inner", "leaf")]).expect("a one-level map"),
    );
    for _ in 0..500_000 {
        mixed = PropertyValue::Array(vec![mixed].into_boxed_slice());
    }
    assert!(PropertyMap::new(vec![Property::new("oem", mixed)]).is_err());
}

fn nest_arrays(depth: u32) -> Result<PropertyMap, PropertyMapError> {
    let mut value = PropertyValue::from("leaf");
    for _ in 0..depth {
        value = PropertyValue::Array(vec![value].into_boxed_slice());
    }
    PropertyMap::new(vec![Property::new("oem", value)])
}

/// Each level is its own map, so every level but the outermost is validated on
/// the way up and only the last can report the whole chain as too deep.
fn nest_objects(depth: u32) -> Result<PropertyMap, PropertyMapError> {
    let mut value = PropertyValue::from("leaf");
    for _ in 0..depth {
        value = PropertyValue::Object(PropertyMap::new(vec![Property::new("n", value)])?);
    }
    PropertyMap::new(vec![Property::new("oem", value)])
}

/// The depth limit is only worth having if what it admits can be read back.
///
/// `PropertyValue` is adjacently tagged, so each level costs a decoder several
/// levels of its own recursion limit, and the batch adds more on top. Without
/// this, `MAX_DEPTH` can drift past what `serde_json` accepts and the crate
/// starts encoding graphs that a consumer of the same crate rejects.
#[cfg(feature = "serde")]
#[test]
fn the_deepest_accepted_property_survives_every_supported_format() {
    for deepest in [
        nest_arrays(PropertyMap::MAX_DEPTH).expect("arrays at the limit"),
        nest_objects(PropertyMap::MAX_DEPTH).expect("objects at the limit"),
    ] {
        let batch = graph_batch(
            ObservationScope::Subject(host_subject()),
            ResourceGraph::new(
                vec![ObservedResource::complete(
                    host_subject(),
                    "/redfish/v1/Systems/host-1",
                    deepest,
                )],
                Vec::new(),
            )
            .expect("a single-resource graph"),
        )
        .expect("a valid batch");

        let json = serde_json::to_string(&batch).expect("encodes as json");
        assert_eq!(
            serde_json::from_str::<ObservationBatch>(&json).expect("decodes from json"),
            batch
        );

        let mut cbor = Vec::new();
        ciborium::into_writer(&batch, &mut cbor).expect("encodes as cbor");
        assert_eq!(
            ciborium::from_reader::<ObservationBatch, _>(cbor.as_slice())
                .expect("decodes from cbor"),
            batch
        );
    }
}

#[test]
fn graph_size_is_bounded_and_the_bound_is_configurable() {
    let oversized = ResourceGraph::with_limits(
        vec![host_resource(), dpu_resource()],
        Vec::new(),
        GraphLimits::new(1, 0),
    );
    assert!(matches!(
        oversized,
        Err(ResourceGraphError::TooManyResources { count: 2, limit: 1 })
    ));

    let too_many_relations = ResourceGraph::with_limits(
        vec![host_resource(), dpu_resource()],
        vec![ResourceRelation::new(
            host_subject(),
            "contains",
            dpu_subject(),
        )],
        GraphLimits::new(10, 0),
    );
    assert!(matches!(
        too_many_relations,
        Err(ResourceGraphError::TooManyRelations { count: 1, limit: 0 })
    ));

    // A graph exactly at the limit is within it.
    assert!(ResourceGraph::with_limits(
        vec![host_resource(), dpu_resource()],
        vec![ResourceRelation::new(
            host_subject(),
            "contains",
            dpu_subject(),
        )],
        GraphLimits::new(2, 1),
    )
    .is_ok());

    // A limit looser than the default is clamped to it, so the model cannot
    // build a graph that its own deserialization would then reject.
    let beyond_default = GraphLimits::new(usize::MAX, usize::MAX);
    assert!(ResourceGraph::with_limits(
        vec![host_resource(), dpu_resource()],
        Vec::new(),
        beyond_default,
    )
    .is_ok());
    assert!(matches!(
        ResourceGraph::with_limits(
            (0..=GraphLimits::DEFAULT.max_resources)
                .map(|index| ObservedResource::complete(
                    Subject::new("node", format!("n{index}")),
                    format!("/nodes/{index}"),
                    PropertyMap::empty(),
                ))
                .collect(),
            Vec::new(),
            beyond_default,
        ),
        Err(ResourceGraphError::TooManyResources { limit, .. })
            if limit == GraphLimits::DEFAULT.max_resources
    ));
}

#[test]
fn graph_sorts_resources_and_relations_and_supports_external_targets() {
    let external_switch = Subject::new("switch", "leaf-1");
    let mut builder = ResourceGraphBuilder::new();
    builder.push_resource(dpu_resource());
    builder.push_resource(host_resource());
    builder.push_relation(ResourceRelation::new(
        host_subject(),
        "connected_to",
        external_switch,
    ));
    builder.push_relation(ResourceRelation::new(
        host_subject(),
        "contains",
        dpu_subject(),
    ));

    let graph = builder.finish().expect("valid graph");

    assert_eq!(&graph.resources()[0].subject, &host_subject());
    assert_eq!(&graph.resources()[1].subject, &dpu_subject());
    assert_eq!(graph.relations()[0].kind.as_str(), "connected_to");
    assert_eq!(graph.relations_from(&host_subject()).len(), 2);
    assert!(graph.relations_from(&dpu_subject()).is_empty());
    assert_eq!(graph.relations_to(&dpu_subject()).count(), 1);
    assert!(graph.get(&dpu_subject()).is_some());
}

#[test]
fn relation_lookup_isolates_each_source_and_traverses_cycles() {
    let manager = Subject::new("manager", "bmc");
    let manager_resource = ObservedResource::complete(
        manager.clone(),
        "/redfish/v1/Managers/BMC",
        PropertyMap::empty(),
    );
    let graph = ResourceGraph::new(
        vec![host_resource(), dpu_resource(), manager_resource],
        vec![
            ResourceRelation::new(host_subject(), "contains", dpu_subject()),
            ResourceRelation::new(host_subject(), "managed_by", manager.clone()),
            // Closes a cycle, which traversal must terminate on.
            ResourceRelation::new(manager.clone(), "manages", host_subject()),
        ],
    )
    .expect("valid graph");

    assert_eq!(graph.relations_from(&host_subject()).len(), 2);
    assert_eq!(graph.relations_from(&manager).len(), 1);
    assert_eq!(graph.relations_from(&manager)[0].kind.as_str(), "manages");
    assert!(graph.relations_from(&dpu_subject()).is_empty());

    let mut reachable = graph.reachable_from(&host_subject());
    reachable.sort();
    assert_eq!(reachable, vec![&host_subject(), &dpu_subject(), &manager]);

    assert_eq!(graph.reachable_from(&dpu_subject()), vec![&dpu_subject()]);
    assert!(graph
        .reachable_from(&Subject::new("switch", "absent"))
        .is_empty());
}

/// Relations are bound to their source by walking both sorted slices at once,
/// so the shapes that matter are where a run starts and ends: the first
/// resource, the last, one with nothing in between, and the empty edges.
#[test]
fn relation_lookup_holds_for_every_shape_of_source_run() {
    let first = Subject::new("chassis", "a");
    let middle = Subject::new("chassis", "m");
    let last = Subject::new("chassis", "z");
    let resource = |subject: &Subject, key: &str| {
        ObservedResource::complete(subject.clone(), key.to_owned(), PropertyMap::empty())
    };
    let resources = || {
        vec![
            resource(&first, "/a"),
            resource(&middle, "/m"),
            resource(&last, "/z"),
        ]
    };

    // The middle resource has no outgoing relations, so both neighbours'
    // runs have to stop at the right place.
    let graph = ResourceGraph::new(
        resources(),
        vec![
            ResourceRelation::new(first.clone(), "contains", middle.clone()),
            ResourceRelation::new(last.clone(), "contains", first.clone()),
            ResourceRelation::new(last.clone(), "peer_of", middle.clone()),
        ],
    )
    .expect("a valid graph");

    assert_eq!(graph.relations_from(&first).len(), 1);
    assert!(graph.relations_from(&middle).is_empty());
    assert_eq!(graph.relations_from(&last).len(), 2);
    assert_eq!(graph.reachable_from(&last), vec![&first, &middle, &last]);
    assert_eq!(graph.first_unreachable_from(&first), Some(&last));
    assert_eq!(graph.first_unreachable_from(&last), None);

    let edgeless = ResourceGraph::new(resources(), Vec::new()).expect("a valid graph");
    assert!(edgeless.relations_from(&first).is_empty());
    assert!(edgeless.relations_from(&last).is_empty());
    assert_eq!(edgeless.first_unreachable_from(&middle), Some(&first));

    let empty = ResourceGraph::empty();
    assert!(empty.relations_from(&first).is_empty());
    assert_eq!(empty.first_unreachable_from(&first), None);
}

/// A relation may point outside the graph, and traversal has to step over one
/// rather than stopping at it.
#[test]
fn traversal_steps_over_relations_leaving_the_graph() {
    let external = Subject::new("switch", "leaf-1");
    let graph = ResourceGraph::new(
        vec![host_resource(), dpu_resource()],
        vec![
            ResourceRelation::new(host_subject(), "connected_to", external),
            ResourceRelation::new(host_subject(), "contains", dpu_subject()),
        ],
    )
    .expect("a valid graph");

    assert_eq!(
        graph.reachable_from(&host_subject()),
        vec![&host_subject(), &dpu_subject()]
    );
    assert_eq!(graph.first_unreachable_from(&host_subject()), None);
}

#[test]
fn identical_graphs_compare_and_hash_equally_regardless_of_insertion_order() {
    let forward = ResourceGraph::new(vec![host_resource(), dpu_resource()], Vec::new())
        .expect("valid forward graph");
    let reversed = ResourceGraph::new(vec![dpu_resource(), host_resource()], Vec::new())
        .expect("valid reversed graph");

    assert_eq!(forward, reversed);
    assert_eq!(hash_of(&forward), hash_of(&reversed));
}

fn hash_of(graph: &ResourceGraph) -> u64 {
    use std::hash::DefaultHasher;
    use std::hash::Hash;
    use std::hash::Hasher;

    let mut hasher = DefaultHasher::new();
    graph.hash(&mut hasher);
    hasher.finish()
}

#[test]
fn completeness_records_whether_a_missing_property_is_meaningful() {
    let complete = host_resource();
    assert_eq!(complete.completeness, ResourceCompleteness::Complete);
    assert!(complete.establishes_absence());

    let selected = ObservedResource::partial(
        host_subject(),
        "/redfish/v1/Systems/host-1",
        properties(vec![Property::new("power_state", "on")]),
    );
    assert!(!selected.establishes_absence());
    assert!(selected.properties.get("bios_settings_hash").is_none());
}

#[test]
fn one_source_location_may_only_back_one_resource() {
    let merged = ResourceGraph::new(
        vec![
            host_resource(),
            ObservedResource::complete(
                Subject::new("computer_system", "host-1-environment"),
                "/redfish/v1/Systems/host-1",
                PropertyMap::empty(),
            ),
        ],
        Vec::new(),
    );

    assert!(matches!(
        merged,
        Err(ResourceGraphError::DuplicateSourceKey(source_key))
            if source_key.as_str() == "/redfish/v1/Systems/host-1"
    ));
}

#[test]
fn graph_rejects_duplicate_resources_relations_and_unknown_sources() {
    let duplicate_resource = ResourceGraph::new(vec![host_resource(), host_resource()], Vec::new());
    assert!(matches!(
        duplicate_resource,
        Err(ResourceGraphError::DuplicateResource(subject))
            if subject == host_subject()
    ));

    let relation = ResourceRelation::new(host_subject(), "contains", dpu_subject());
    let duplicate_relation = ResourceGraph::new(
        vec![host_resource(), dpu_resource()],
        vec![relation.clone(), relation],
    );
    assert!(matches!(
        duplicate_relation,
        Err(ResourceGraphError::DuplicateRelation { .. })
    ));

    let unknown_source = ResourceGraph::new(
        vec![host_resource()],
        vec![ResourceRelation::new(
            Subject::new("manager", "missing"),
            "manages",
            host_subject(),
        )],
    );
    assert!(matches!(
        unknown_source,
        Err(ResourceGraphError::UnknownRelationSource(subject))
            if subject == Subject::new("manager", "missing")
    ));
}

fn graph_batch(
    scope: ObservationScope,
    graph: ResourceGraph,
) -> Result<ObservationBatch, BatchError> {
    ObservationBatch::new(
        endpoint(),
        Origin::new("redfish", "resource-graph"),
        ObservationWindow::point(timestamp()),
        Coverage::new(scope, nv_telemetry_core::Completeness::Complete),
        Payload::Resources(graph),
    )
}

fn host_contains_dpu() -> ResourceGraph {
    ResourceGraph::new(
        vec![host_resource(), dpu_resource()],
        vec![ResourceRelation::new(
            host_subject(),
            "contains",
            dpu_subject(),
        )],
    )
    .expect("valid graph")
}

#[test]
fn payload_length_counts_relations_as_well_as_resources() {
    let batch = graph_batch(ObservationScope::Endpoint, host_contains_dpu())
        .expect("endpoint scope accepts any graph");

    assert_eq!(batch.payload().len(), 3);
}

#[test]
fn subject_scope_accepts_the_subtree_rooted_at_the_scope_subject() {
    let batch = graph_batch(
        ObservationScope::Subject(host_subject()),
        host_contains_dpu(),
    )
    .expect("the dpu is reachable from the host root");

    assert_eq!(
        batch.coverage().scope,
        ObservationScope::Subject(host_subject())
    );
}

#[test]
fn subject_scope_rejects_resources_outside_the_rooted_subtree() {
    let disconnected =
        ResourceGraph::new(vec![host_resource(), dpu_resource()], Vec::new()).expect("valid graph");

    assert!(matches!(
        graph_batch(ObservationScope::Subject(host_subject()), disconnected),
        Err(BatchError::UnreachableFromScopeRoot { subject, .. }) if subject == dpu_subject()
    ));
}

/// Rejection has to cost what the traversal costs, not a scan per resource.
///
/// `ObservationBatch::new` runs this on the way in from `Deserialize`, so an
/// endpoint chooses how often it happens and picks the graph it happens on.
/// Naming the unreachable resource by searching the reachable set for a
/// resource it omits is quadratic, which at the default limit of 100,000
/// resources takes tens of seconds per rejected batch.
#[test]
fn rejecting_a_large_graph_costs_what_the_traversal_costs() {
    const RESOURCES: usize = 20_000;

    let root = Subject::new("chassis", "root");
    let node = |index: usize| Subject::new("node", format!("n{index:06}"));
    let mut resources = vec![ObservedResource::complete(
        root.clone(),
        "/redfish/v1/Chassis/root",
        PropertyMap::empty(),
    )];
    let mut relations = Vec::new();
    for index in 0..RESOURCES {
        resources.push(ObservedResource::complete(
            node(index),
            format!("/redfish/v1/Chassis/root/Nodes/{index}"),
            PropertyMap::empty(),
        ));
        // Every node but the last hangs off the root.
        if index + 1 < RESOURCES {
            relations.push(ResourceRelation::new(root.clone(), "contains", node(index)));
        }
    }
    let graph = ResourceGraph::new(resources, relations).expect("a valid graph");

    let started = Instant::now();
    let rejected = graph_batch(ObservationScope::Subject(root), graph);
    let elapsed = started.elapsed();

    assert!(matches!(
        rejected,
        Err(BatchError::UnreachableFromScopeRoot { subject, .. })
            if subject == node(RESOURCES - 1)
    ));
    // Linear rejection lands in milliseconds even unoptimized, and the
    // quadratic form takes minutes, so the bound only has to separate those
    // two rather than measure anything.
    assert!(
        elapsed < Duration::from_secs(30),
        "rejecting {RESOURCES} resources took {elapsed:?}, which suggests the \
         scan-per-resource form is back"
    );
}

/// A link the collector cannot yet name is a property, not an edge.
///
/// Both ends of a relation are subjects, so stating one asserts an identity.
/// A collector that has only a location keeps it as a reference until it can
/// resolve it, because an invented subject would be indistinguishable from a
/// genuine external target.
#[test]
fn an_unresolved_link_stays_a_reference_until_its_subject_is_known() {
    const LINK: &str = "/redfish/v1/Chassis/1/NetworkAdapters/1";

    let unresolved = ResourceGraph::new(
        vec![ObservedResource::complete(
            host_subject(),
            "/redfish/v1/Systems/host-1",
            PropertyMap::new(vec![Property::new(
                "network_adapter",
                ResourceReference::new(LINK),
            )])
            .expect("unique property names"),
        )],
        Vec::new(),
    )
    .expect("a valid graph");

    let reference = match unresolved
        .get(&host_subject())
        .expect("the host is in the graph")
        .properties
        .get("network_adapter")
    {
        Some(PropertyValue::Reference(reference)) => reference,
        other => panic!("expected a reference, got {other:?}"),
    };
    assert_eq!(reference.source_key.as_str(), LINK);
    assert_eq!(reference.subject, None, "nothing has resolved it yet");
    assert_eq!(unresolved.relation_count(), 0);

    // Once the identity is known the link becomes an edge, and the target
    // need not have been collected for the graph to accept it.
    let adapter = Subject::new("network_adapter", "1");
    let resolved = ResourceGraph::new(
        vec![ObservedResource::complete(
            host_subject(),
            "/redfish/v1/Systems/host-1",
            PropertyMap::new(vec![Property::new(
                "network_adapter",
                ResourceReference::new(LINK).with_subject(adapter.clone()),
            )])
            .expect("unique property names"),
        )],
        vec![ResourceRelation::new(
            host_subject(),
            "contains",
            adapter.clone(),
        )],
    )
    .expect("an external target is allowed");

    assert_eq!(resolved.relations_from(&host_subject()).len(), 1);
    assert_eq!(resolved.relations_from(&host_subject())[0].target, adapter);
    assert!(
        resolved.get(&adapter).is_none(),
        "the edge names the adapter without the graph holding it"
    );
}

#[test]
fn subject_scope_requires_its_root_to_be_present() {
    let without_root = ResourceGraph::new(vec![dpu_resource()], Vec::new()).expect("valid graph");

    assert!(matches!(
        graph_batch(ObservationScope::Subject(host_subject()), without_root),
        Err(BatchError::MissingScopeRoot(root)) if root == host_subject()
    ));

    // An empty graph observes nothing, so it cannot contradict a scope.
    graph_batch(
        ObservationScope::Subject(host_subject()),
        ResourceGraph::empty(),
    )
    .expect("an empty graph is in scope everywhere");
}

#[test]
fn relation_identity_ignores_relation_properties() {
    let labelled = |label| {
        ResourceRelation::new(host_subject(), "contains", dpu_subject())
            .with_properties(properties(vec![Property::new("discovered_by", label)]))
    };

    let graph = ResourceGraph::new(
        vec![host_resource(), dpu_resource()],
        vec![labelled("chassis-walk"), labelled("systems-walk")],
    );

    assert!(matches!(
        graph,
        Err(ResourceGraphError::DuplicateRelation { kind, .. }) if kind.as_str() == "contains"
    ));

    let single = ResourceGraph::new(
        vec![host_resource(), dpu_resource()],
        vec![labelled("chassis-walk")],
    )
    .expect("one edge is not a duplicate");
    assert_eq!(
        single.relations()[0].properties.get("discovered_by"),
        Some(&PropertyValue::from("chassis-walk"))
    );
}

#[cfg(feature = "serde")]
#[test]
fn serde_round_trip_preserves_validated_resource_graph_batch() {
    let batch = graph_batch(ObservationScope::Endpoint, host_contains_dpu()).expect("valid batch");

    let encoded = serde_json::to_string(&batch).expect("serialize graph batch");
    let decoded: ObservationBatch =
        serde_json::from_str(&encoded).expect("deserialize graph batch");

    assert_eq!(decoded, batch);
}

#[cfg(feature = "serde")]
#[test]
fn bytes_round_trip_as_hex_rather_than_a_numeric_array() {
    let map = PropertyMap::new(vec![Property::new(
        "certificate",
        PropertyValue::Bytes(vec![0x00, 0x1f, 0xab, 0xff].into()),
    )])
    .expect("unique property names");

    let encoded = serde_json::to_string(&map).expect("serialize bytes");
    assert!(
        encoded.contains("001fabff"),
        "unexpected encoding: {encoded}"
    );
    assert_eq!(
        serde_json::from_str::<PropertyMap>(&encoded).expect("deserialize bytes"),
        map
    );
}

#[cfg(feature = "serde")]
#[test]
fn bytes_stay_native_in_a_binary_format() {
    let blob: Vec<u8> = (0..=u8::MAX).collect();
    let map = PropertyMap::new(vec![Property::new(
        "certificate",
        PropertyValue::Bytes(blob.clone().into()),
    )])
    .expect("unique property names");

    let mut encoded = Vec::new();
    ciborium::into_writer(&map, &mut encoded).expect("serialize bytes");
    assert_eq!(
        ciborium::from_reader::<PropertyMap, _>(encoded.as_slice()).expect("deserialize bytes"),
        map
    );

    let hex_width = blob.len() * 2;
    assert!(
        encoded.len() < hex_width,
        "binary format paid for hex: {} bytes for a {}-byte blob",
        encoded.len(),
        blob.len()
    );
}

#[cfg(feature = "serde")]
#[test]
fn serde_rejects_invalid_resource_graph_and_nested_property_values() {
    let resource = host_resource();
    let duplicate_graph = serde_json::json!({
        "resources": [resource.clone(), resource],
        "relations": []
    });
    assert!(serde_json::from_value::<ResourceGraph>(duplicate_graph).is_err());

    let duplicate_properties = serde_json::json!([
        {"name": "state", "value": {"type": "string", "value": "enabled"}},
        {"name": "state", "value": {"type": "string", "value": "disabled"}}
    ]);
    assert!(serde_json::from_value::<PropertyMap>(duplicate_properties).is_err());

    let non_finite = serde_json::json!({"type": "f64", "value": null});
    assert!(serde_json::from_value::<PropertyValue>(non_finite).is_err());
}
