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

mod common;

use common::endpoint;
use common::hash_of;
use common::timestamp;
use nv_telemetry_core::BatchError;
use nv_telemetry_core::Coverage;
use nv_telemetry_core::DurationValue;
use nv_telemetry_core::GraphLimits;
use nv_telemetry_core::ObservationBatch;
use nv_telemetry_core::ObservationScope;
use nv_telemetry_core::ObservationWindow;
use nv_telemetry_core::ObservedResource;
use nv_telemetry_core::Origin;
use nv_telemetry_core::Payload;
use nv_telemetry_core::Property;
use nv_telemetry_core::PropertyArray;
use nv_telemetry_core::PropertyArrayError;
use nv_telemetry_core::PropertyMap;
use nv_telemetry_core::PropertyMapError;
use nv_telemetry_core::PropertyValue;
use nv_telemetry_core::Reachability;
use nv_telemetry_core::RelationKind;
use nv_telemetry_core::ResourceCompleteness;
use nv_telemetry_core::ResourceGraph;
use nv_telemetry_core::ResourceGraphError;
use nv_telemetry_core::ResourceReference;
use nv_telemetry_core::ResourceRelation;
use nv_telemetry_core::SourceKey;
use nv_telemetry_core::Subject;
use nv_telemetry_core::SubjectId;
use nv_telemetry_core::SubjectKind;

/// The Unix second the fixtures are taken to have been observed at.
const OBSERVED_AT: i64 = 1_721_000_000;

fn subject(kind: impl Into<SubjectKind>, id: impl Into<SubjectId>) -> Subject {
    Subject::new(kind.into(), id.into())
}

fn source_key(value: impl Into<SourceKey>) -> SourceKey {
    value.into()
}

fn relation(source: Subject, kind: impl Into<RelationKind>, target: Subject) -> ResourceRelation {
    ResourceRelation::new(source, kind.into(), target)
}

fn host_subject() -> Subject {
    subject("computer_system", "host-1")
}

fn dpu_subject() -> Subject {
    subject("dpu", "dpu-0")
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
            PropertyValue::array(vec![
                PropertyValue::from("192.0.2.10"),
                PropertyValue::from("2001:db8::10"),
            ])
            .expect("shallow property array"),
        ),
        Property::new("connected", true),
    ]);
    ObservedResource::complete(
        host_subject(),
        source_key("/redfish/v1/Systems/host-1"),
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
    .with_observed_at(timestamp(OBSERVED_AT))
}

fn dpu_resource() -> ObservedResource {
    ObservedResource::complete(
        dpu_subject(),
        source_key("/redfish/v1/Systems/host-1/Processors/DPU0"),
        properties(vec![
            Property::new("agent_version", "3.2.1"),
            Property::new(
                "manager",
                ResourceReference::new(source_key("/redfish/v1/Managers/BMC"))
                    .with_subject(subject("manager", "bmc")),
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

    // A stored array is read back through the same surface a consumer walking
    // a device's list-valued property has: the wrapper is a slice everywhere
    // it can be, and only the depth check stands between a `Vec` and one.
    let addresses = match nested(&map, "network").get("addresses") {
        Some(PropertyValue::Array(addresses)) => addresses.clone(),
        other => panic!("expected an array of addresses, got {other:?}"),
    };
    let expected = [
        PropertyValue::from("192.0.2.10"),
        PropertyValue::from("2001:db8::10"),
    ];

    assert_eq!(addresses.len(), 2);
    assert!(!addresses.is_empty());
    assert_eq!(addresses.as_slice(), expected);
    assert_eq!(addresses.as_ref(), expected, "AsRef agrees with as_slice");
    assert!(addresses
        .iter()
        .all(|address| matches!(address, PropertyValue::String(_))));

    let mut borrowed = Vec::new();
    for address in &addresses {
        borrowed.push(address.clone());
    }
    assert_eq!(borrowed.as_slice(), expected, "a borrowed array iterates");

    // Handing the values back out keeps their order, and a caller's own `Vec`
    // is admitted by the depth rule the stored array was built under.
    let recovered = addresses.into_vec();
    assert_eq!(recovered.as_slice(), expected);
    assert!(PropertyArray::try_from(Vec::new())
        .expect("no values to nest")
        .is_empty());
    assert_eq!(
        PropertyValue::from(PropertyArray::try_from(recovered).expect("a shallow array")),
        PropertyValue::array(Vec::from(expected)).expect("a shallow array"),
        "the conversion decoding goes through admits what the constructor does"
    );
    assert_eq!(
        PropertyArray::try_from(vec![
            nested_array(PropertyMap::MAX_DEPTH).expect("arrays at the limit")
        ]),
        Err(PropertyArrayError::DepthExceeded {
            limit: PropertyMap::MAX_DEPTH,
        })
    );
}

/// Reads a nested object out of a map, failing rather than reading as absent.
fn nested<'map>(map: &'map PropertyMap, name: &str) -> &'map PropertyMap {
    match map.get(name) {
        Some(PropertyValue::Object(nested)) => nested,
        other => panic!("expected a nested object at {name}, got {other:?}"),
    }
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
    // A bare value may nest through the whole budget, whichever container
    // spends it.
    assert!(nested_array(PropertyMap::MAX_DEPTH).is_ok());
    assert_eq!(
        nested_array(PropertyMap::MAX_DEPTH + 1),
        Err(PropertyArrayError::DepthExceeded {
            limit: PropertyMap::MAX_DEPTH,
        })
    );
    assert!(nested_object(PropertyMap::MAX_DEPTH).is_ok());
    assert!(matches!(
        nested_object(PropertyMap::MAX_DEPTH + 1),
        Err(PropertyMapError::DepthExceeded { name, .. }) if name.as_str() == "n"
    ));

    // The map a value is stored in spends one of those levels itself, so what
    // it accepts is one shallower.
    assert!(nest_arrays(PropertyMap::MAX_DEPTH - 1).is_ok());
    assert!(matches!(
        nest_arrays(PropertyMap::MAX_DEPTH),
        Err(PropertyMapError::DepthExceeded { name, .. }) if name.as_str() == "oem"
    ));
    assert!(nest_objects(PropertyMap::MAX_DEPTH - 1).is_ok());
    assert!(matches!(
        nest_objects(PropertyMap::MAX_DEPTH),
        Err(PropertyMapError::DepthExceeded { name, .. }) if name.as_str() == "oem"
    ));

    // Arrays reject the first level that would make recursive clone, hash,
    // equality, serialization, or drop exceed the shared bound.
    assert!(nested_array(500_000).is_err());

    let mixed = PropertyValue::Object(
        PropertyMap::new(vec![Property::new("inner", "leaf")]).expect("a one-level map"),
    );
    assert_eq!(
        nested_array_from(mixed, 500_000),
        Err(PropertyArrayError::DepthExceeded {
            limit: PropertyMap::MAX_DEPTH,
        })
    );
}

fn nest_arrays(depth: u32) -> Result<PropertyMap, PropertyMapError> {
    let value = nested_array(depth).expect("requested depth is within the property limit");
    PropertyMap::new(vec![Property::new("oem", value)])
}

fn nested_array(depth: u32) -> Result<PropertyValue, PropertyArrayError> {
    nested_array_from(PropertyValue::from("leaf"), depth)
}

fn nested_array_from(
    mut value: PropertyValue,
    depth: u32,
) -> Result<PropertyValue, PropertyArrayError> {
    for _ in 0..depth {
        value = PropertyValue::array(vec![value])?;
    }
    Ok(value)
}

fn nest_objects(depth: u32) -> Result<PropertyMap, PropertyMapError> {
    let value = nested_object(depth).expect("requested depth is within the property limit");
    PropertyMap::new(vec![Property::new("oem", value)])
}

/// Each level is its own map, so every level but the outermost is validated on
/// the way up and only the last can report the whole chain as too deep.
fn nested_object(depth: u32) -> Result<PropertyValue, PropertyMapError> {
    let mut value = PropertyValue::from("leaf");
    for _ in 0..depth {
        value = PropertyValue::Object(PropertyMap::new(vec![Property::new("n", value)])?);
    }
    Ok(value)
}

/// Every value the constructors accept has to decode with nothing around it.
///
/// `PropertyValue` and `PropertyMap` admit a value without reference to what
/// encloses it, so the decoder has to admit it with nothing enclosing it
/// either. Checking only the in-a-graph case hides an off-by-one between
/// construction and decoding behind the levels the graph itself spends.
#[cfg(feature = "serde")]
#[test]
fn the_deepest_accepted_value_survives_a_standalone_round_trip() {
    for deepest in [
        nested_array(PropertyMap::MAX_DEPTH).expect("arrays at the limit"),
        nested_object(PropertyMap::MAX_DEPTH).expect("objects at the limit"),
    ] {
        let json = serde_json::to_string(&deepest).expect("encodes as json");
        assert_eq!(
            serde_json::from_str::<PropertyValue>(&json).expect("decodes from json"),
            deepest
        );

        let mut cbor = Vec::new();
        ciborium::into_writer(&deepest, &mut cbor).expect("encodes as cbor");
        assert_eq!(
            ciborium::from_reader::<PropertyValue, _>(cbor.as_slice()).expect("decodes from cbor"),
            deepest
        );

        let messagepack = rmp_serde::to_vec(&deepest).expect("encodes as messagepack");
        assert_eq!(
            rmp_serde::from_slice::<PropertyValue>(&messagepack).expect("decodes from messagepack"),
            deepest
        );
    }
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
        nest_arrays(PropertyMap::MAX_DEPTH - 1).expect("arrays at the limit"),
        nest_objects(PropertyMap::MAX_DEPTH - 1).expect("objects at the limit"),
    ] {
        let batch = graph_batch(
            ObservationScope::Subject(host_subject()),
            ResourceGraph::new(
                vec![ObservedResource::complete(
                    host_subject(),
                    source_key("/redfish/v1/Systems/host-1"),
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
        GraphLimits::default()
            .with_max_resources(1)
            .with_max_relations(0),
    );
    assert!(matches!(
        oversized,
        Err(ResourceGraphError::TooManyResources { count: 2, limit: 1 })
    ));

    let too_many_relations = ResourceGraph::with_limits(
        vec![host_resource(), dpu_resource()],
        vec![relation(host_subject(), "contains", dpu_subject())],
        GraphLimits::default()
            .with_max_resources(10)
            .with_max_relations(0),
    );
    assert!(matches!(
        too_many_relations,
        Err(ResourceGraphError::TooManyRelations { count: 1, limit: 0 })
    ));

    // A graph exactly at the limit is within it.
    assert!(ResourceGraph::with_limits(
        vec![host_resource(), dpu_resource()],
        vec![relation(host_subject(), "contains", dpu_subject(),)],
        GraphLimits::default()
            .with_max_resources(2)
            .with_max_relations(1),
    )
    .is_ok());

    // A limit looser than the default is clamped to it, so the model cannot
    // build a graph that its own deserialization would then reject. The
    // clamped value is what the accessors report, too.
    let beyond_default = GraphLimits::default()
        .with_max_resources(usize::MAX)
        .with_max_relations(usize::MAX);
    assert_eq!(
        beyond_default.max_resources(),
        GraphLimits::DEFAULT.max_resources()
    );
    assert_eq!(
        beyond_default.max_relations(),
        GraphLimits::DEFAULT.max_relations()
    );
    assert!(ResourceGraph::with_limits(
        vec![host_resource(), dpu_resource()],
        Vec::new(),
        beyond_default,
    )
    .is_ok());
    assert!(matches!(
        ResourceGraph::with_limits(
            (0..=GraphLimits::DEFAULT.max_resources())
                .map(|index| ObservedResource::complete(
                    subject("node", format!("n{index}")),
                    source_key(format!("/nodes/{index}")),
                    PropertyMap::empty(),
                ))
                .collect(),
            Vec::new(),
            beyond_default,
        ),
        Err(ResourceGraphError::TooManyResources { limit, .. })
            if limit == GraphLimits::DEFAULT.max_resources()
    ));

    // The relation ceiling is clamped the same way. Size is checked before
    // anything is sorted, so the edges need not be distinct or resolvable.
    let edge = relation(host_subject(), "contains", dpu_subject());
    assert!(matches!(
        ResourceGraph::with_limits(
            vec![host_resource(), dpu_resource()],
            vec![edge; GraphLimits::DEFAULT.max_relations() + 1],
            beyond_default,
        ),
        Err(ResourceGraphError::TooManyRelations { limit, .. })
            if limit == GraphLimits::DEFAULT.max_relations()
    ));
}

#[test]
fn graph_sorts_resources_and_relations_and_supports_external_targets() {
    let external_switch = subject("switch", "leaf-1");
    let graph = ResourceGraph::new(
        vec![dpu_resource(), host_resource()],
        vec![
            relation(host_subject(), "connected_to", external_switch),
            relation(host_subject(), "contains", dpu_subject()),
        ],
    )
    .expect("valid graph");

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
    let manager = subject("manager", "bmc");
    let manager_resource = ObservedResource::complete(
        manager.clone(),
        source_key("/redfish/v1/Managers/BMC"),
        PropertyMap::empty(),
    );
    let graph = ResourceGraph::new(
        vec![host_resource(), dpu_resource(), manager_resource],
        vec![
            relation(host_subject(), "contains", dpu_subject()),
            relation(host_subject(), "managed_by", manager.clone()),
            // Closes a cycle, which traversal must terminate on.
            relation(manager.clone(), "manages", host_subject()),
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
        .reachable_from(&subject("switch", "absent"))
        .is_empty());
}

#[test]
fn traversal_reports_a_detached_resource_beside_a_cycle() {
    let manager = subject("manager", "detached");
    let graph = ResourceGraph::new(
        vec![
            host_resource(),
            dpu_resource(),
            ObservedResource::complete(
                manager.clone(),
                source_key("/redfish/v1/Managers/detached"),
                PropertyMap::empty(),
            ),
        ],
        vec![
            relation(host_subject(), "contains", dpu_subject()),
            relation(dpu_subject(), "part_of", host_subject()),
        ],
    )
    .expect("cycle and detached resource form a valid graph");

    assert_eq!(
        graph.reachability_from(&host_subject()),
        Reachability::Unreachable(&manager)
    );
}

/// Relations are bound to their source by walking both sorted slices at once,
/// so the shapes that matter are where a run starts and ends: the first
/// resource, the last, one with nothing in between, and the empty edges.
#[test]
fn relation_lookup_holds_for_every_shape_of_source_run() {
    let first = subject("chassis", "a");
    let middle = subject("chassis", "m");
    let last = subject("chassis", "z");
    let resource = |subject: &Subject, key: &str| {
        ObservedResource::complete(
            subject.clone(),
            source_key(key.to_owned()),
            PropertyMap::empty(),
        )
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
            relation(first.clone(), "contains", middle.clone()),
            relation(last.clone(), "contains", first.clone()),
            relation(last.clone(), "peer_of", middle.clone()),
        ],
    )
    .expect("a valid graph");

    assert_eq!(graph.relations_from(&first).len(), 1);
    assert!(graph.relations_from(&middle).is_empty());
    assert_eq!(graph.relations_from(&last).len(), 2);
    assert_eq!(graph.reachable_from(&last), vec![&first, &middle, &last]);
    assert_eq!(
        graph.reachability_from(&first),
        Reachability::Unreachable(&last)
    );
    assert_eq!(graph.reachability_from(&last), Reachability::FullyReachable);

    let edgeless = ResourceGraph::new(resources(), Vec::new()).expect("a valid graph");
    assert!(edgeless.relations_from(&first).is_empty());
    assert!(edgeless.relations_from(&last).is_empty());
    assert_eq!(
        edgeless.reachability_from(&middle),
        Reachability::Unreachable(&first)
    );

    let empty = ResourceGraph::empty();
    assert!(empty.relations_from(&first).is_empty());
    assert_eq!(empty.reachability_from(&first), Reachability::MissingRoot);
}

/// A relation may point outside the graph, and traversal has to step over one
/// rather than stopping at it.
#[test]
fn traversal_steps_over_relations_leaving_the_graph() {
    let external = subject("switch", "leaf-1");
    let graph = ResourceGraph::new(
        vec![host_resource(), dpu_resource()],
        vec![
            relation(host_subject(), "connected_to", external),
            relation(host_subject(), "contains", dpu_subject()),
        ],
    )
    .expect("a valid graph");

    assert_eq!(
        graph.reachable_from(&host_subject()),
        vec![&host_subject(), &dpu_subject()]
    );
    assert_eq!(
        graph.reachability_from(&host_subject()),
        Reachability::FullyReachable
    );
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

#[test]
fn completeness_records_whether_a_missing_property_is_meaningful() {
    let complete = host_resource();
    assert_eq!(complete.completeness, ResourceCompleteness::Complete);
    assert!(complete.completeness.establishes_absence());

    let selected = ObservedResource::partial(
        host_subject(),
        source_key("/redfish/v1/Systems/host-1"),
        properties(vec![Property::new("power_state", "on")]),
    );
    assert!(!selected.completeness.establishes_absence());
    assert!(selected.properties.get("bios_settings_hash").is_none());
}

#[test]
fn one_source_location_may_only_back_one_resource() {
    let merged = ResourceGraph::new(
        vec![
            host_resource(),
            ObservedResource::complete(
                subject("computer_system", "host-1-environment"),
                source_key("/redfish/v1/Systems/host-1"),
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

    let edge = relation(host_subject(), "contains", dpu_subject());
    let duplicate_relation = ResourceGraph::new(
        vec![host_resource(), dpu_resource()],
        vec![edge.clone(), edge],
    );
    assert!(matches!(
        duplicate_relation,
        Err(ResourceGraphError::DuplicateRelation { .. })
    ));

    let unknown_source = ResourceGraph::new(
        vec![host_resource()],
        vec![relation(
            subject("manager", "missing"),
            "manages",
            host_subject(),
        )],
    );
    assert!(matches!(
        unknown_source,
        Err(ResourceGraphError::UnknownRelationSource(missing))
            if missing == subject("manager", "missing")
    ));
}

fn graph_batch(
    scope: ObservationScope,
    graph: ResourceGraph,
) -> Result<ObservationBatch, BatchError> {
    ObservationBatch::new(
        endpoint(),
        Origin::new("redfish".into(), "resource-graph".into()),
        ObservationWindow::point(timestamp(OBSERVED_AT)),
        Coverage::new(scope, nv_telemetry_core::Completeness::Complete),
        Payload::Resources(graph),
    )
}

fn host_contains_dpu() -> ResourceGraph {
    ResourceGraph::new(
        vec![host_resource(), dpu_resource()],
        vec![relation(host_subject(), "contains", dpu_subject())],
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

/// Rejection has to name the resource that fell outside the scope, however
/// large the graph is.
///
/// What that costs is gated separately by the `reject_scope` benchmark, whose
/// instruction counts do not depend on machine load.
#[test]
fn rejecting_a_large_graph_names_the_unreachable_resource() {
    const RESOURCES: usize = 20_000;

    let root = subject("chassis", "root");
    let node = |index: usize| subject("node", format!("n{index:06}"));
    let mut resources = vec![ObservedResource::complete(
        root.clone(),
        source_key("/redfish/v1/Chassis/root"),
        PropertyMap::empty(),
    )];
    let mut relations = Vec::new();
    for index in 0..RESOURCES {
        resources.push(ObservedResource::complete(
            node(index),
            source_key(format!("/redfish/v1/Chassis/root/Nodes/{index}")),
            PropertyMap::empty(),
        ));
        // Every node but the last hangs off the root.
        if index + 1 < RESOURCES {
            relations.push(relation(root.clone(), "contains", node(index)));
        }
    }
    let graph = ResourceGraph::new(resources, relations).expect("a valid graph");

    assert!(matches!(
        graph_batch(ObservationScope::Subject(root), graph),
        Err(BatchError::UnreachableFromScopeRoot { subject, .. })
            if subject == node(RESOURCES - 1)
    ));
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
            source_key("/redfish/v1/Systems/host-1"),
            PropertyMap::new(vec![Property::new(
                "network_adapter",
                ResourceReference::new(source_key(LINK)),
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
    let adapter = subject("network_adapter", "1");
    let resolved = ResourceGraph::new(
        vec![ObservedResource::complete(
            host_subject(),
            source_key("/redfish/v1/Systems/host-1"),
            PropertyMap::new(vec![Property::new(
                "network_adapter",
                ResourceReference::new(source_key(LINK)).with_subject(adapter.clone()),
            )])
            .expect("unique property names"),
        )],
        vec![relation(host_subject(), "contains", adapter.clone())],
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
        relation(host_subject(), "contains", dpu_subject())
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

    assert_ne!(
        labelled("chassis-walk"),
        labelled("systems-walk"),
        "edge properties participate in value equality even though they are not identity"
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
fn default_messagepack_round_trip_accepts_contentless_null_only() {
    let sensor = subject("sensor", "CPU0Temp");
    let batch = graph_batch(
        ObservationScope::Endpoint,
        ResourceGraph::new(
            vec![ObservedResource::complete(
                sensor,
                source_key("/redfish/v1/Chassis/1/Sensors/CPU0Temp"),
                properties(vec![
                    Property::new("lower_caution", PropertyValue::Null),
                    Property::new("nullable_oem_value", PropertyValue::Null),
                    Property::new("upper_critical", PropertyValue::Null),
                ]),
            )],
            Vec::new(),
        )
        .expect("a valid graph"),
    )
    .expect("an endpoint-scoped graph");

    // The default MessagePack serializer represents structs as sequences. A
    // unit variant's adjacent tag is therefore a one-element sequence with no
    // content slot.
    let encoded = rmp_serde::to_vec(&batch).expect("serialize graph batch as MessagePack");
    assert_eq!(
        rmp_serde::from_slice::<ObservationBatch>(&encoded)
            .expect("deserialize graph batch from MessagePack"),
        batch
    );

    for value_bearing_tag in [
        "bool",
        "i64",
        "u64",
        "f64",
        "string",
        "bytes",
        "timestamp",
        "duration",
        "reference",
        "array",
        "object",
    ] {
        let missing_content =
            rmp_serde::to_vec(&[value_bearing_tag]).expect("encode one-element sequence");
        assert!(
            rmp_serde::from_slice::<PropertyValue>(&missing_content).is_err(),
            "accepted missing content for the {value_bearing_tag} tag"
        );
    }
}

#[cfg(feature = "serde")]
#[test]
fn semantic_source_and_relation_types_keep_string_wire_fields() {
    let resource = serde_json::to_value(host_resource()).expect("serialize resource");
    assert_eq!(
        resource["source_key"],
        serde_json::json!("/redfish/v1/Systems/host-1")
    );

    let edge = serde_json::to_value(relation(host_subject(), "contains", dpu_subject()))
        .expect("serialize relation");
    assert_eq!(edge["kind"], serde_json::json!("contains"));

    let reference = serde_json::to_value(ResourceReference::new(source_key(
        "/redfish/v1/Managers/BMC",
    )))
    .expect("serialize reference");
    assert_eq!(
        reference["source_key"],
        serde_json::json!("/redfish/v1/Managers/BMC")
    );
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
    assert_eq!(
        serde_json::from_str::<PropertyValue>(r#"{"value":"001fabff","type":"bytes"}"#)
            .expect("deserialize content-first hex bytes"),
        PropertyValue::Bytes(vec![0x00, 0x1f, 0xab, 0xff].into())
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
fn malformed_hex_and_numeric_json_sequences_are_rejected() {
    for malformed in [
        r#"{"type":"bytes","value":"0"}"#,
        r#"{"type":"bytes","value":"0g"}"#,
        r#"{"type":"bytes","value":[0,1,255]}"#,
    ] {
        assert!(
            serde_json::from_str::<PropertyValue>(malformed).is_err(),
            "accepted malformed byte value: {malformed}"
        );
    }
}

#[cfg(feature = "serde")]
#[test]
fn binary_byte_sequences_are_accepted_without_changing_the_wire_variant() {
    use ciborium::value::Integer;
    use ciborium::value::Value;

    for value_first in [false, true] {
        let tag = (Value::Text("type".into()), Value::Text("bytes".into()));
        let content = (
            Value::Text("value".into()),
            Value::Array(vec![
                Value::Integer(Integer::from(0_u8)),
                Value::Integer(Integer::from(1_u8)),
                Value::Integer(Integer::from(255_u8)),
            ]),
        );
        let entries = if value_first {
            vec![content, tag]
        } else {
            vec![tag, content]
        };
        let mut encoded = Vec::new();
        ciborium::into_writer(&Value::Map(entries), &mut encoded).expect("encode CBOR sequence");

        assert_eq!(
            ciborium::from_reader::<PropertyValue, _>(encoded.as_slice())
                .expect("decode binary byte sequence"),
            PropertyValue::Bytes(vec![0, 1, 255].into())
        );
    }
}

#[cfg(feature = "serde")]
#[test]
fn standalone_property_deserialization_enforces_the_depth_budget() {
    let nested_array = |depth: u32| {
        let mut value = serde_json::json!({"type": "string", "value": "leaf"});
        for _ in 0..depth {
            value = serde_json::json!({"type": "array", "value": [value]});
        }
        value
    };

    assert!(serde_json::from_value::<PropertyValue>(nested_array(PropertyMap::MAX_DEPTH)).is_ok());
    assert!(
        serde_json::from_value::<PropertyValue>(nested_array(PropertyMap::MAX_DEPTH + 1)).is_err()
    );
}

/// A `value` read before its `type` decodes as the same value.
///
/// The two orders take different paths: a tag already read decodes the payload
/// straight into its variant, while a tag still to come leaves the payload
/// buffered and replayed. The container tags are covered by the content-first
/// depth tests and `bytes` and `string` by the hex tests, which leaves the
/// remaining leaves to pin here. Each needs both paths held to the same value,
/// or one of them can drift into reading the same bytes differently.
#[cfg(feature = "serde")]
#[test]
fn content_first_json_decodes_each_leaf_tag_as_its_tag_first_spelling() {
    let reference = ResourceReference::new(source_key("/redfish/v1/Managers/BMC"));
    for value in [
        PropertyValue::Null,
        PropertyValue::Bool(true),
        PropertyValue::I64(-7),
        PropertyValue::U64(7),
        PropertyValue::f64(1.5).expect("a finite value"),
        PropertyValue::Timestamp(timestamp(OBSERVED_AT)),
        PropertyValue::Duration(DurationValue::new(-3, 500).expect("a valid duration")),
        PropertyValue::Reference(reference.clone()),
        PropertyValue::Reference(reference.clone().with_subject(subject("manager", "bmc"))),
    ] {
        // The payload comes from the encoder rather than being spelled out, so
        // the two orders cannot be compared against a stale hand-written shape.
        let encoded = serde_json::to_value(&value).expect("serialize property value");
        let tag = encoded["type"].as_str().expect("an encoded tag");
        // A unit variant carries no content, and `null` is what a format
        // writes for the payload it does have.
        let content = encoded
            .get("value")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let content = serde_json::to_string(&content).expect("serialize the content");

        for spelling in [
            format!(r#"{{"type":"{tag}","value":{content}}}"#),
            format!(r#"{{"value":{content},"type":"{tag}"}}"#),
        ] {
            assert_eq!(
                serde_json::from_str::<PropertyValue>(&spelling)
                    .unwrap_or_else(|error| panic!("{spelling} was rejected: {error}")),
                value,
                "{spelling}"
            );
        }
    }
}

#[cfg(feature = "serde")]
#[test]
fn content_first_json_obeys_the_same_property_depth_budget() {
    for at_limit in [
        content_first_array_json(PropertyMap::MAX_DEPTH),
        content_first_object_json(PropertyMap::MAX_DEPTH),
    ] {
        serde_json::from_str::<PropertyValue>(&at_limit)
            .unwrap_or_else(|error| panic!("content-first JSON at the limit failed: {error}"));
    }

    let over_limit = content_first_array_json(PropertyMap::MAX_DEPTH + 1);
    let error = serde_json::from_str::<PropertyValue>(&over_limit)
        .expect_err("content-first JSON over the model limit must be rejected");
    assert!(
        error.to_string().contains("nests deeper than the limit"),
        "the semantic depth guard did not reject the value: {error}"
    );
}

#[cfg(feature = "serde")]
#[test]
fn content_first_binary_input_is_bounded_before_its_tag() {
    for at_limit in [
        content_first_array_cbor(PropertyMap::MAX_DEPTH),
        content_first_object_cbor(PropertyMap::MAX_DEPTH),
    ] {
        ciborium::from_reader::<PropertyValue, _>(at_limit.as_slice())
            .unwrap_or_else(|error| panic!("content-first CBOR at the limit failed: {error}"));
    }

    let over_limit = content_first_array_cbor(PropertyMap::MAX_DEPTH + 1);
    let error = ciborium::from_reader::<PropertyValue, _>(over_limit.as_slice())
        .expect_err("content-first CBOR over the model limit must be rejected");
    assert!(
        error.to_string().contains("nests deeper than the limit"),
        "the semantic depth guard did not reject the CBOR value: {error}"
    );

    // This reaches the pre-tag structural guard, rather than recursively
    // buffering or dropping the complete attacker-controlled value.
    let far_over_limit = content_first_array_cbor(100_000);
    let error = ciborium::from_reader::<PropertyValue, _>(far_over_limit.as_slice())
        .expect_err("extreme content-first CBOR must be rejected");
    assert!(
        error.to_string().contains("before its type tag"),
        "the bounded pre-tag buffer did not reject extreme nesting: {error}"
    );

    // A failed decode must restore the thread-local semantic budget.
    let shallow = content_first_array_cbor(1);
    assert!(
        ciborium::from_reader::<PropertyValue, _>(shallow.as_slice()).is_ok(),
        "a rejected value must not poison later decoding"
    );
}

#[cfg(feature = "serde")]
fn content_first_array_json(depth: u32) -> String {
    let mut encoded = String::new();
    for _ in 0..depth {
        encoded.push_str(r#"{"value":["#);
    }
    encoded.push_str(r#"{"value":"leaf","type":"string"}"#);
    for _ in 0..depth {
        encoded.push_str(r#"],"type":"array"}"#);
    }
    encoded
}

#[cfg(feature = "serde")]
fn content_first_object_json(depth: u32) -> String {
    let mut encoded = String::new();
    for _ in 0..depth {
        encoded.push_str(r#"{"value":[{"name":"n","value":"#);
    }
    encoded.push_str(r#"{"value":"leaf","type":"string"}"#);
    for _ in 0..depth {
        encoded.push_str(r#"}],"type":"object"}"#);
    }
    encoded
}

#[cfg(feature = "serde")]
fn content_first_array_cbor(depth: u32) -> Vec<u8> {
    const ARRAY_PREFIX: &[u8] = b"\xa2\x65value\x81";
    const ARRAY_SUFFIX: &[u8] = b"\x64type\x65array";
    const STRING_LEAF: &[u8] = b"\xa2\x65value\x64leaf\x64type\x66string";

    let layer_width = ARRAY_PREFIX.len() + ARRAY_SUFFIX.len();
    let mut encoded = Vec::with_capacity(depth as usize * layer_width + STRING_LEAF.len());
    for _ in 0..depth {
        encoded.extend_from_slice(ARRAY_PREFIX);
    }
    encoded.extend_from_slice(STRING_LEAF);
    for _ in 0..depth {
        encoded.extend_from_slice(ARRAY_SUFFIX);
    }
    encoded
}

#[cfg(feature = "serde")]
fn content_first_object_cbor(depth: u32) -> Vec<u8> {
    const OBJECT_PREFIX: &[u8] = b"\xa2\x65value\x81\xa2\x64name\x61n\x65value";
    const OBJECT_SUFFIX: &[u8] = b"\x64type\x66object";
    const STRING_LEAF: &[u8] = b"\xa2\x65value\x64leaf\x64type\x66string";

    let layer_width = OBJECT_PREFIX.len() + OBJECT_SUFFIX.len();
    let mut encoded = Vec::with_capacity(depth as usize * layer_width + STRING_LEAF.len());
    for _ in 0..depth {
        encoded.extend_from_slice(OBJECT_PREFIX);
    }
    encoded.extend_from_slice(STRING_LEAF);
    for _ in 0..depth {
        encoded.extend_from_slice(OBJECT_SUFFIX);
    }
    encoded
}

#[cfg(feature = "serde")]
#[test]
fn graph_deserialization_applies_default_resource_limits() {
    use std::fmt::Write as _;

    let mut encoded = String::from(r#"{"resources":["#);
    for index in 0..=GraphLimits::DEFAULT.max_resources() {
        if index != 0 {
            encoded.push(',');
        }
        write!(
            encoded,
            r#"{{"subject":{{"kind":"node","id":"n{index}"}},"source_key":"/nodes/{index}","completeness":"complete","schema":null,"version":null,"observed_at":null,"properties":[]}}"#
        )
        .expect("write to string");
    }
    encoded.push_str(r#"],"relations":[]}"#);

    let error = serde_json::from_str::<ResourceGraph>(&encoded)
        .expect_err("one resource over the default must be rejected");
    assert!(error.to_string().contains("exceeding the limit"));
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

/// A property value is as strict about its fields as every other record.
#[cfg(feature = "serde")]
#[test]
fn serde_rejects_an_undeclared_field_beside_a_property_value() {
    for stray in [
        serde_json::json!({"type": "i64", "value": 1, "unit": "Cel"}),
        serde_json::json!({"unit": "Cel", "type": "i64", "value": 1}),
    ] {
        let error = serde_json::from_value::<PropertyValue>(stray)
            .expect_err("an undeclared field must be rejected");
        assert!(
            error.to_string().contains("unknown field `unit`"),
            "the field was accepted and discarded instead: {error}"
        );
    }
}
