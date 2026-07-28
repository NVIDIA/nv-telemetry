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
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct GraphLimits {
    pub max_resources: usize,
    pub max_relations: usize,
}

impl GraphLimits {
    /// Generous bounds intended to catch runaway input, not to size a budget.
    pub const DEFAULT: Self = Self {
        max_resources: 100_000,
        max_relations: 500_000,
    };

    /// Applies no bound, for callers that enforce their own.
    ///
    /// Deserialization always applies [`DEFAULT`](Self::DEFAULT), so a graph
    /// built past those bounds serializes but will not decode.
    pub const UNLIMITED: Self = Self {
        max_resources: usize::MAX,
        max_relations: usize::MAX,
    };

    pub const fn new(max_resources: usize, max_relations: usize) -> Self {
        Self {
            max_resources,
            max_relations,
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
        // Checked before sorting so oversized input costs nothing.
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

        if let Some(duplicate) = sort_and_find_duplicate(&mut resources, |left, right| {
            left.subject.cmp(&right.subject)
        }) {
            return Err(ResourceGraphError::DuplicateResource(
                duplicate.subject.clone(),
            ));
        }

        let mut source_keys: Vec<&Name> = resources
            .iter()
            .map(|resource| &resource.source_key)
            .collect();
        if let Some(duplicate) = sort_and_find_duplicate(&mut source_keys, Ord::cmp) {
            let duplicate = (*duplicate).clone();
            return Err(ResourceGraphError::DuplicateSourceKey(duplicate));
        }

        if let Some(duplicate) = sort_and_find_duplicate(&mut relations, compare_relations) {
            return Err(ResourceGraphError::DuplicateRelation {
                source: duplicate.source.clone(),
                kind: duplicate.kind.clone(),
                target: duplicate.target.clone(),
            });
        }

        // Both sides are now sorted on the same key, so one merge pass finds a
        // source the graph does not hold. A relation left behind by the walk
        // sorts before every remaining resource and so can never match one.
        let mut relation = 0;
        for resource in &resources {
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
        if let Some(orphan) = relations.get(relation) {
            return Err(ResourceGraphError::UnknownRelationSource(
                orphan.source.clone(),
            ));
        }

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
        self.index_of(subject).map(|index| &self.resources[index])
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
            .into_iter()
            .zip(&self.resources)
            .filter_map(|(seen, resource)| seen.then_some(&resource.subject))
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
        let visited = self.visit_from(root);
        let index = visited.iter().position(|seen| !seen)?;
        Some(&self.resources[index].subject)
    }

    /// Marks every resource reachable from the resource at `root`.
    fn visit_from(&self, root: usize) -> Vec<bool> {
        let bounds = self.relation_bounds();
        let mut visited = vec![false; self.resources.len()];
        visited[root] = true;
        let mut pending = vec![root];
        while let Some(index) = pending.pop() {
            for relation in &self.relations[bounds[index]..bounds[index + 1]] {
                let Some(target) = self.index_of(&relation.target) else {
                    continue;
                };
                if std::mem::replace(&mut visited[target], true) {
                    continue;
                }
                pending.push(target);
            }
        }
        visited
    }

    /// Returns where each resource's outgoing relations start, plus a final
    /// entry holding the relation count.
    ///
    /// Resolving a subject to its relations costs a string comparison per
    /// probe, which a traversal would otherwise pay at every node it visits.
    /// Both slices are sorted on the same key, so one merge pass bounds every
    /// run at once. Sources are known to exist, having been checked during
    /// assembly, so the walk never has to skip a relation.
    fn relation_bounds(&self) -> Vec<usize> {
        let mut bounds = Vec::with_capacity(self.resources.len() + 1);
        let mut relation = 0;
        for resource in &self.resources {
            bounds.push(relation);
            while self
                .relations
                .get(relation)
                .is_some_and(|next| next.source == resource.subject)
            {
                relation += 1;
            }
        }
        bounds.push(relation);
        bounds
    }

    fn index_of(&self, subject: &Subject) -> Option<usize> {
        self.resources
            .binary_search_by(|resource| resource.subject.cmp(subject))
            .ok()
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

fn compare_relations(left: &ResourceRelation, right: &ResourceRelation) -> Ordering {
    left.source
        .cmp(&right.source)
        .then_with(|| left.kind.cmp(&right.kind))
        .then_with(|| left.target.cmp(&right.target))
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

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for ResourceGraph {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Representation {
            resources: Vec<ObservedResource>,
            relations: Vec<ResourceRelation>,
        }

        let value = <Representation as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(value.resources, value.relations).map_err(serde::de::Error::custom)
    }
}
