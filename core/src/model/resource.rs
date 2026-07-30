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

use std::cmp::Ordering;
use std::error::Error;
use std::fmt;
use std::ops::Range;

use super::collection::sort_and_find_duplicate;
use super::Name;
use super::PropertyMap;
use super::Subject;
use super::Timestamp;

/// Whether a resource's property map is the device's full representation.
///
/// In a [`Complete`] resource, absence proves the device does not implement
/// the property. In a [`Partial`] resource, absence proves nothing: it may
/// never have been requested. A consumer that conflates the two would read an
/// uncollected property as unset and try to write it.
///
/// [`Complete`]: ResourceCompleteness::Complete
/// [`Partial`]: ResourceCompleteness::Partial
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[non_exhaustive]
pub enum ResourceCompleteness {
    /// The full representation was observed; absence is meaningful.
    Complete,
    /// A subset was observed; absence carries no information.
    Partial,
}

/// One protocol-neutral resource observed on an endpoint.
///
/// A resource corresponds to exactly one source location, identified by
/// `source_key`. Data from elsewhere belongs in its own resource joined by a
/// [`ResourceRelation`]; merging would collapse two fetch times and two entity
/// tags into one.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ObservedResource {
    pub subject: Subject,
    /// The single source location this resource was read from.
    pub source_key: Name,
    pub completeness: ResourceCompleteness,
    pub schema: Option<Name>,
    pub version: Option<Name>,
    pub observed_at: Option<Timestamp>,
    pub properties: PropertyMap,
}

impl ObservedResource {
    /// Records a resource whose full representation was observed.
    pub fn complete(
        subject: Subject,
        source_key: impl Into<Name>,
        properties: PropertyMap,
    ) -> Self {
        Self::new(
            subject,
            source_key,
            ResourceCompleteness::Complete,
            properties,
        )
    }

    /// Records a resource whose properties are a subset of the representation.
    pub fn partial(subject: Subject, source_key: impl Into<Name>, properties: PropertyMap) -> Self {
        Self::new(
            subject,
            source_key,
            ResourceCompleteness::Partial,
            properties,
        )
    }

    pub fn new(
        subject: Subject,
        source_key: impl Into<Name>,
        completeness: ResourceCompleteness,
        properties: PropertyMap,
    ) -> Self {
        Self {
            subject,
            source_key: source_key.into(),
            completeness,
            schema: None,
            version: None,
            observed_at: None,
            properties,
        }
    }

    #[must_use]
    pub fn with_schema(mut self, schema: impl Into<Name>) -> Self {
        self.schema = Some(schema.into());
        self
    }

    #[must_use]
    pub fn with_version(mut self, version: impl Into<Name>) -> Self {
        self.version = Some(version.into());
        self
    }

    #[must_use]
    pub fn with_observed_at(mut self, observed_at: Timestamp) -> Self {
        self.observed_at = Some(observed_at);
        self
    }

    /// Returns whether absence of a property is meaningful for this resource.
    pub fn establishes_absence(&self) -> bool {
        matches!(self.completeness, ResourceCompleteness::Complete)
    }
}

/// One directed, typed relationship in an observed resource graph.
///
/// Identity is `(source, kind, target)`. Properties describe an edge but do
/// not distinguish two of them, so a graph holding the same triple twice is
/// rejected however their properties differ.
///
/// Both ends are [`Subject`]s, the canonical identity of a thing, not the
/// location a link arrived as. A relation therefore asserts a resolved fact
/// about topology. The target still need not be in the graph, so asserting one
/// costs no fetch: what it requires is knowing the target's identity, not
/// having collected it.
///
/// A collector holding only a link has two ways to get there. Where the
/// source's naming is canonical it derives the subject from the link itself,
/// as a Redfish path names its collection and id. Where it is not, the link is
/// not yet a relation: it belongs on the resource as a
/// [`PropertyValue::Reference`], which carries the location and leaves
/// [`ResourceReference::subject`] empty, and a later pass that learns the
/// identity promotes it. Inventing a subject to make an edge out of an
/// unresolved link would produce one that no resource ever matches, which the
/// graph cannot tell from a genuine external target.
///
/// [`PropertyValue::Reference`]: super::PropertyValue::Reference
/// [`ResourceReference::subject`]: super::ResourceReference::subject
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ResourceRelation {
    pub source: Subject,
    pub kind: Name,
    pub target: Subject,
    pub properties: PropertyMap,
}

impl ResourceRelation {
    pub fn new(source: Subject, kind: impl Into<Name>, target: Subject) -> Self {
        Self {
            source,
            kind: kind.into(),
            target,
            properties: PropertyMap::empty(),
        }
    }

    #[must_use]
    pub fn with_properties(mut self, properties: PropertyMap) -> Self {
        self.properties = properties;
        self
    }
}

/// Immutable snapshot of observed resources and their relationships.
///
/// Resources are sorted by subject and relations by `(source, kind, target)`,
/// so two graphs with the same content compare and hash identically
/// regardless of discovery order.
///
/// Relation sources must be present in the graph. Targets may be external, so
/// that a partial graph keeps its links into scopes that were not collected.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "GraphParts"))]
pub struct ResourceGraph {
    resources: Box<[ObservedResource]>,
    relations: Box<[ResourceRelation]>,
}

/// Upper bounds applied when assembling a [`ResourceGraph`].
///
/// A malfunctioning or hostile endpoint can advertise an unbounded topology.
/// The check happens once the input is collected, so it bounds what a graph
/// can hold rather than what a caller can allocate on the way there; a source
/// reading an endpoint incrementally should stop itself rather than fill a
/// [`ResourceGraphBuilder`] and learn at `finish`.
///
/// Limits only ever tighten [`DEFAULT`](Self::DEFAULT). Deserialization has no
/// caller to take them from and so applies the default, and a graph built past
/// it would encode into a payload this crate then refuses to read. A bound
/// above the default is clamped rather than honoured, so whatever the model
/// accepts, it can also read back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct GraphLimits {
    pub max_resources: usize,
    pub max_relations: usize,
}

impl GraphLimits {
    /// Generous bounds intended to catch runaway input, not to size a budget.
    ///
    /// Also the ceiling, since this is what deserialization applies.
    pub const DEFAULT: Self = Self {
        max_resources: 100_000,
        max_relations: 500_000,
    };

    pub const fn new(max_resources: usize, max_relations: usize) -> Self {
        Self {
            max_resources,
            max_relations,
        }
    }

    /// Returns these limits with anything looser than the default pulled back.
    const fn clamped(self) -> Self {
        Self {
            max_resources: if self.max_resources > Self::DEFAULT.max_resources {
                Self::DEFAULT.max_resources
            } else {
                self.max_resources
            },
            max_relations: if self.max_relations > Self::DEFAULT.max_relations {
                Self::DEFAULT.max_relations
            } else {
                self.max_relations
            },
        }
    }
}

impl Default for GraphLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl ResourceGraph {
    /// Assembles a graph under [`GraphLimits::DEFAULT`].
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`with_limits`](Self::with_limits).
    pub fn new(
        resources: Vec<ObservedResource>,
        relations: Vec<ResourceRelation>,
    ) -> Result<Self, ResourceGraphError> {
        Self::with_limits(resources, relations, GraphLimits::DEFAULT)
    }

    /// Assembles a graph under caller-supplied limits.
    ///
    /// # Errors
    ///
    /// Returns [`ResourceGraphError::DuplicateResource`] or
    /// [`ResourceGraphError::DuplicateSourceKey`] if one subject or one source
    /// location is described twice, [`ResourceGraphError::DuplicateRelation`]
    /// for a repeated edge, [`ResourceGraphError::UnknownRelationSource`] if an
    /// edge leaves a subject the graph does not hold, and
    /// [`ResourceGraphError::TooManyResources`] or
    /// [`ResourceGraphError::TooManyRelations`] if the graph exceeds `limits`.
    pub fn with_limits(
        mut resources: Vec<ObservedResource>,
        mut relations: Vec<ResourceRelation>,
        limits: GraphLimits,
    ) -> Result<Self, ResourceGraphError> {
        check_size(&resources, &relations, limits.clamped())?;
        sort_unique_subjects(&mut resources)?;
        check_unique_source_keys(&resources)?;
        sort_unique_relations(&mut relations)?;
        // Both sides are sorted on the same key by the two steps above, which
        // is what lets this one be a single merge pass.
        check_relation_sources(&resources, &relations)?;

        Ok(Self {
            resources: resources.into_boxed_slice(),
            relations: relations.into_boxed_slice(),
        })
    }

    pub fn empty() -> Self {
        Self::default()
    }

    pub fn resources(&self) -> &[ObservedResource] {
        &self.resources
    }

    pub fn relations(&self) -> &[ResourceRelation] {
        &self.relations
    }

    pub fn get(&self, subject: &Subject) -> Option<&ObservedResource> {
        self.index_of(subject).map(|index| self.resource(index))
    }

    /// Returns the outgoing relations of `subject` in `O(log n)`.
    pub fn relations_from(&self, subject: &Subject) -> &[ResourceRelation] {
        let start = self
            .relations
            .partition_point(|relation| relation.source < *subject);
        let end = self.relations[start..].partition_point(|relation| relation.source == *subject);
        &self.relations[start..start + end]
    }

    /// Returns the incoming relations of `subject`.
    ///
    /// Relations are ordered by source, so this scans the whole relation list.
    /// Callers that traverse upward repeatedly should build their own index.
    pub fn relations_to<'graph, 'subject>(
        &'graph self,
        subject: &'subject Subject,
    ) -> impl Iterator<Item = &'graph ResourceRelation> + use<'graph, 'subject> {
        self.relations
            .iter()
            .filter(move |relation| &relation.target == subject)
    }

    /// Returns the subjects reachable from `root` by following relations.
    ///
    /// Traversal follows edges from source to target only, so a subtree must
    /// be linked by outgoing relations from its root such as `contains`; the
    /// model has no notion of which relation kinds invert. Relations pointing
    /// outside the graph are ignored, and the root is included when present.
    /// Subjects come back in graph order rather than traversal order.
    pub fn reachable_from(&self, root: &Subject) -> Vec<&Subject> {
        let Some(root) = self.index_of(root) else {
            return Vec::new();
        };

        self.visit_from(root)
            .reached()
            .map(|index| &self.resource(index).subject)
            .collect()
    }

    /// Returns the first resource in graph order that `root` cannot reach.
    ///
    /// Answering this by searching [`reachable_from`](Self::reachable_from)
    /// for a resource it omits costs a scan of the reachable set per resource.
    /// Scope validation asks the question about graphs an endpoint controls,
    /// and asks it only when the graph is being rejected, so the answer has to
    /// cost the same as the traversal itself.
    ///
    /// Returns `None` when `root` is absent, since a graph missing its own
    /// root has no resource to single out.
    pub fn first_unreachable_from(&self, root: &Subject) -> Option<&Subject> {
        let root = self.index_of(root)?;
        let unreached = self.visit_from(root).first_unreached()?;
        Some(&self.resource(unreached).subject)
    }

    /// Marks every resource reachable from the resource at `root`.
    fn visit_from(&self, root: ResourceIndex) -> Reached {
        let bounds = RelationBounds::of(self);
        let mut reached = Reached::none(self.resources.len());
        reached.mark(root);

        let mut pending = vec![root];
        while let Some(index) = pending.pop() {
            for relation in &self.relations[bounds.of_resource(index)] {
                let Some(target) = self.index_of(&relation.target) else {
                    continue;
                };
                if reached.mark(target) {
                    pending.push(target);
                }
            }
        }
        reached
    }

    fn index_of(&self, subject: &Subject) -> Option<ResourceIndex> {
        self.resources
            .binary_search_by(|resource| resource.subject.cmp(subject))
            .ok()
            .map(ResourceIndex)
    }

    fn resource(&self, index: ResourceIndex) -> &ObservedResource {
        &self.resources[index.0]
    }

    pub const fn resource_count(&self) -> usize {
        self.resources.len()
    }

    pub const fn relation_count(&self) -> usize {
        self.relations.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }
}

/// A resource's position in the graph's subject-sorted resource list.
///
/// A traversal carries positions in both the resource list and the relation
/// list, and as bare integers the two were interchangeable. This one indexes
/// resources; [`RelationBounds`] converts it to the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ResourceIndex(usize);

/// Which resources a traversal reached, indexed by [`ResourceIndex`].
#[derive(Debug)]
struct Reached(Vec<bool>);

impl Reached {
    fn none(resources: usize) -> Self {
        Self(vec![false; resources])
    }

    /// Marks `index`, returning whether that was news.
    fn mark(&mut self, index: ResourceIndex) -> bool {
        !std::mem::replace(&mut self.0[index.0], true)
    }

    fn reached(&self) -> impl Iterator<Item = ResourceIndex> + '_ {
        self.0
            .iter()
            .enumerate()
            .filter_map(|(index, reached)| reached.then_some(ResourceIndex(index)))
    }

    fn first_unreached(&self) -> Option<ResourceIndex> {
        self.0
            .iter()
            .position(|reached| !reached)
            .map(ResourceIndex)
    }
}

/// Where each resource's outgoing relations sit in the relation list.
///
/// Resolving a subject to its relations costs a string comparison per probe,
/// which a traversal would otherwise pay at every node it visits. Both slices
/// are sorted on the same key, so one merge pass bounds every run at once.
/// Sources are known to exist, having been checked during assembly, so the
/// walk never has to skip a relation.
#[derive(Debug)]
struct RelationBounds(Vec<usize>);

impl RelationBounds {
    fn of(graph: &ResourceGraph) -> Self {
        let mut starts = Vec::with_capacity(graph.resources.len() + 1);
        let mut relation = 0;
        for resource in &graph.resources {
            starts.push(relation);
            while graph
                .relations
                .get(relation)
                .is_some_and(|next| next.source == resource.subject)
            {
                relation += 1;
            }
        }
        starts.push(relation);
        Self(starts)
    }

    fn of_resource(&self, index: ResourceIndex) -> Range<usize> {
        self.0[index.0]..self.0[index.0 + 1]
    }
}

fn compare_relations(left: &ResourceRelation, right: &ResourceRelation) -> Ordering {
    left.source
        .cmp(&right.source)
        .then_with(|| left.kind.cmp(&right.kind))
        .then_with(|| left.target.cmp(&right.target))
}

/// Rejects input past `limits` before anything has been sorted, so oversized
/// input costs no more than counting it.
fn check_size(
    resources: &[ObservedResource],
    relations: &[ResourceRelation],
    limits: GraphLimits,
) -> Result<(), ResourceGraphError> {
    if resources.len() > limits.max_resources {
        return Err(ResourceGraphError::TooManyResources {
            count: resources.len(),
            limit: limits.max_resources,
        });
    }
    if relations.len() > limits.max_relations {
        return Err(ResourceGraphError::TooManyRelations {
            count: relations.len(),
            limit: limits.max_relations,
        });
    }
    Ok(())
}

/// Orders resources by subject, rejecting a subject described twice.
fn sort_unique_subjects(resources: &mut [ObservedResource]) -> Result<(), ResourceGraphError> {
    match sort_and_find_duplicate(resources, |left, right| left.subject.cmp(&right.subject)) {
        Some(duplicate) => Err(ResourceGraphError::DuplicateResource(
            duplicate.subject.clone(),
        )),
        None => Ok(()),
    }
}

/// Rejects one source location described by two resources.
fn check_unique_source_keys(resources: &[ObservedResource]) -> Result<(), ResourceGraphError> {
    let mut source_keys: Vec<&Name> = resources
        .iter()
        .map(|resource| &resource.source_key)
        .collect();
    match sort_and_find_duplicate(&mut source_keys, Ord::cmp) {
        Some(duplicate) => Err(ResourceGraphError::DuplicateSourceKey(Name::clone(
            duplicate,
        ))),
        None => Ok(()),
    }
}

/// Orders relations by source, then kind, then target, rejecting a repeat.
fn sort_unique_relations(relations: &mut [ResourceRelation]) -> Result<(), ResourceGraphError> {
    match sort_and_find_duplicate(relations, compare_relations) {
        Some(duplicate) => Err(ResourceGraphError::DuplicateRelation {
            source: duplicate.source.clone(),
            kind: duplicate.kind.clone(),
            target: duplicate.target.clone(),
        }),
        None => Ok(()),
    }
}

/// Rejects an edge leaving a subject the graph does not hold.
///
/// Both slices arrive sorted on the subject, so one merge pass answers this.
/// A relation the walk leaves behind sorts before every remaining resource
/// and so can never match one.
fn check_relation_sources(
    resources: &[ObservedResource],
    relations: &[ResourceRelation],
) -> Result<(), ResourceGraphError> {
    let mut relation = 0;
    for resource in resources {
        while let Some(next) = relations.get(relation) {
            match next.source.cmp(&resource.subject) {
                Ordering::Less => {
                    return Err(ResourceGraphError::UnknownRelationSource(
                        next.source.clone(),
                    ));
                }
                Ordering::Equal => relation += 1,
                Ordering::Greater => break,
            }
        }
    }
    match relations.get(relation) {
        Some(orphan) => Err(ResourceGraphError::UnknownRelationSource(
            orphan.source.clone(),
        )),
        None => Ok(()),
    }
}

/// Mutable assembly boundary for an immutable [`ResourceGraph`].
#[derive(Debug, Default)]
pub struct ResourceGraphBuilder {
    resources: Vec<ObservedResource>,
    relations: Vec<ResourceRelation>,
}

impl ResourceGraphBuilder {
    pub const fn new() -> Self {
        Self {
            resources: Vec::new(),
            relations: Vec::new(),
        }
    }

    pub fn with_capacity(resource_capacity: usize, relation_capacity: usize) -> Self {
        Self {
            resources: Vec::with_capacity(resource_capacity),
            relations: Vec::with_capacity(relation_capacity),
        }
    }

    pub fn push_resource(&mut self, resource: ObservedResource) {
        self.resources.push(resource);
    }

    pub fn push_relation(&mut self, relation: ResourceRelation) {
        self.relations.push(relation);
    }

    pub const fn resource_len(&self) -> usize {
        self.resources.len()
    }

    pub const fn relation_len(&self) -> usize {
        self.relations.len()
    }

    /// Validates the accumulated resources under [`GraphLimits::DEFAULT`].
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`ResourceGraph::with_limits`].
    pub fn finish(self) -> Result<ResourceGraph, ResourceGraphError> {
        ResourceGraph::new(self.resources, self.relations)
    }

    /// Validates the accumulated resources under caller-supplied limits.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`ResourceGraph::with_limits`].
    pub fn finish_with_limits(
        self,
        limits: GraphLimits,
    ) -> Result<ResourceGraph, ResourceGraphError> {
        ResourceGraph::with_limits(self.resources, self.relations, limits)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResourceGraphError {
    DuplicateResource(Subject),
    DuplicateSourceKey(Name),
    DuplicateRelation {
        source: Subject,
        kind: Name,
        target: Subject,
    },
    UnknownRelationSource(Subject),
    TooManyResources {
        count: usize,
        limit: usize,
    },
    TooManyRelations {
        count: usize,
        limit: usize,
    },
}

impl fmt::Display for ResourceGraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateResource(subject) => write!(
                formatter,
                "duplicate resource '{}:{}'",
                subject.kind, subject.id
            ),
            Self::DuplicateSourceKey(source_key) => write!(
                formatter,
                "source '{source_key}' is observed by more than one resource"
            ),
            Self::DuplicateRelation {
                source,
                kind,
                target,
            } => write!(
                formatter,
                "duplicate relation '{}:{}' -[{kind}]-> '{}:{}'",
                source.kind, source.id, target.kind, target.id
            ),
            Self::UnknownRelationSource(subject) => write!(
                formatter,
                "relation source '{}:{}' is not present in the resource graph",
                subject.kind, subject.id
            ),
            Self::TooManyResources { count, limit } => write!(
                formatter,
                "graph has {count} resources, exceeding the limit of {limit}"
            ),
            Self::TooManyRelations { count, limit } => write!(
                formatter,
                "graph has {count} relations, exceeding the limit of {limit}"
            ),
        }
    }
}

impl Error for ResourceGraphError {}

/// The unvalidated parts a [`ResourceGraph`] is assembled from.
#[cfg_attr(feature = "serde", derive(serde::Deserialize))]
struct GraphParts {
    resources: Vec<ObservedResource>,
    relations: Vec<ResourceRelation>,
}

impl TryFrom<GraphParts> for ResourceGraph {
    type Error = ResourceGraphError;

    fn try_from(value: GraphParts) -> Result<Self, Self::Error> {
        Self::new(value.resources, value.relations)
    }
}
