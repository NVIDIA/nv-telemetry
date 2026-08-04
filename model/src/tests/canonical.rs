// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Canonical order and the content digest: input order vanishes, collection
//! metadata breaks ties without moving hashes, the stream is injective where
//! concatenation would lie, and the public order on identities is the
//! canonical order.

use prost::Message as _;

use super::boundary::built_key;
use super::boundary::built_subject;
use crate::generated::wire;
use crate::ObservedResource;
use crate::ResourceGraph;
use crate::SignalKey;
use crate::Subject;
use crate::Value;
use crate::Violation;

/// A hasher that keeps the bytes: hash tests assert on the digest stream
/// itself, which is the contract, rather than on any hash function's output.
#[derive(Default)]
pub(super) struct Collect(pub(super) Vec<u8>);

impl std::hash::Hasher for Collect {
    fn write(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    fn finish(&self) -> u64 {
        0
    }
}

pub(super) fn digest_bytes(graph: &ResourceGraph) -> Vec<u8> {
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
    // agree by construction: the generator emits `Ord` for ordered types as a
    // delegation to `canonical_cmp` rather than a derive, so this is a
    // regression pin on that delegation, not a coincidence held by a test.
    // Scoped and unscoped subjects both, because the agreement must cover
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
