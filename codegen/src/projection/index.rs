// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The Redfish backend's schema index: a thin view over nv-redfish's
//! `SchemaQuery` for checking, plus the same compiled fold held directly
//! for emission.
//!
//! The query answers what a path *is* — type, nullability, cardinality,
//! vocabulary — which is everything the lint judges. Emission additionally
//! needs what the source crate's generated Rust *does* with each segment:
//! which type in the collapsed base chain declares it (so field access
//! spells the `base` hops), whether generation gave it presence-only or
//! nullable shape, and a type definition's underlying primitive. The query
//! does not expose those, so [`RedfishIndex::steps`] walks the same
//! `compile_all` + optimizer fold the query runs and cross-checks every
//! leaf against the query's own resolution — a disagreement on any fact
//! the query states is a build error, never a silent divergence. The
//! fold-only facts have no counterpart to compare against; `steps`
//! documents that boundary.

use std::collections::HashMap;
use std::fmt;
use std::fs;

use nv_redfish_csdl_compiler::compiler::Compiled;
use nv_redfish_csdl_compiler::compiler::Config;
use nv_redfish_csdl_compiler::compiler::EntityTypeFilter;
use nv_redfish_csdl_compiler::compiler::Properties;
use nv_redfish_csdl_compiler::compiler::Property;
use nv_redfish_csdl_compiler::compiler::QualifiedName;
use nv_redfish_csdl_compiler::compiler::SchemaBundle;
use nv_redfish_csdl_compiler::compiler::TypeClass;
use nv_redfish_csdl_compiler::edmx::Edmx;
use nv_redfish_csdl_compiler::optimizer::optimize;
use nv_redfish_csdl_compiler::optimizer::Config as OptimizerConfig;
use nv_redfish_csdl_compiler::query::SchemaQuery;
use nv_redfish_csdl_compiler::OneOrCollection;

/// Why the CSDL bundle could not become an index.
#[derive(Debug)]
pub enum IndexError {
    Io(String, std::io::Error),
    Parse(String, String),
    Build(String),
}

impl fmt::Display for IndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(path, error) => write!(f, "{path}: {error}"),
            Self::Parse(path, error) => write!(f, "{path}: not valid CSDL: {error}"),
            Self::Build(error) => write!(f, "the CSDL bundle does not compile: {error}"),
        }
    }
}

impl std::error::Error for IndexError {}

/// One resolved source field: what the lint type-checks against and the
/// emitter picks conversions from.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ResolvedField {
    /// As CSDL spells it: `Edm.Decimal`, or a reference like `Resource.Health`.
    pub type_name: String,
    pub class: TypeClass,
    pub nullable: bool,
    pub collection: bool,
    /// Member names when the type is an enum: the closed vocabulary
    /// `value_map` and `known_values` are validated against.
    pub enum_members: Option<Vec<String>>,
}

impl ResolvedField {
    /// Whether one value of this field is a single scalar — a primitive,
    /// a type definition, or an enum, as opposed to a structured type.
    #[must_use]
    pub fn is_scalar(&self) -> bool {
        self.class != TypeClass::ComplexType
    }
}

/// One segment of a resolved source path, as the source crate's generated
/// Rust presents it. The last step is the leaf the mapping extracts.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Step {
    /// The segment, in CSDL spelling.
    pub property: String,
    /// `base` hops from the holding type to the declaring type in the
    /// optimizer-collapsed chain — the count of `.base` accesses generation
    /// puts in front of the field.
    pub hops: usize,
    /// `Redfish.Required` on the property; with `nullable`, decides the
    /// generated field's `Option` nesting.
    pub required: bool,
    pub nullable: bool,
    pub collection: bool,
    pub class: TypeClass,
    /// Namespace of the property's type after the fold: `Sensor`, `Edm`.
    pub namespace: String,
    /// Type name within that namespace: `ReadingType`, `Decimal`.
    pub name: String,
    /// For a type definition, the `Edm` primitive underneath — the fact the
    /// query does not expose and conversion selection needs.
    pub underlying: Option<String>,
    /// Member names when the type is an enum.
    pub enum_members: Option<Vec<String>>,
}

impl Step {
    /// How generation shapes the field, from the same rule the source
    /// crate's own struct generator applies.
    #[must_use]
    pub fn shape(&self) -> Shape {
        match (self.required, self.nullable) {
            (true, false) => Shape::Bare,
            (true, true) => Shape::RequiredNullable,
            (false, true) => Shape::Nullable,
            (false, false) => Shape::Optional,
        }
    }

    /// Whether the leaf is text in the generated Rust — `Edm.String` or a
    /// type definition over it, cloned rather than copied. One
    /// classification for conversion selection and field access alike, so
    /// the two cannot diverge.
    #[must_use]
    pub fn is_text(&self) -> bool {
        match self.class {
            TypeClass::SimpleType => self.name == "String",
            TypeClass::TypeDefinition => self.underlying.as_deref() == Some("String"),
            TypeClass::EnumType | TypeClass::ComplexType => false,
        }
    }
}

/// The `Option` nesting generation gives a singular field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// `T`: required and non-nullable.
    Bare,
    /// `Option<T>`: presence only.
    Optional,
    /// `Option<T>`: required on the wire, where `None` is explicit null.
    RequiredNullable,
    /// `Option<Option<T>>`: outer absence, inner explicit null.
    Nullable,
}

/// The parsed bundle; [`RedfishIndex`] borrows from it.
pub struct Bundle {
    bundle: SchemaBundle,
}

impl fmt::Debug for Bundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bundle({} documents)", self.bundle.edmx_docs.len())
    }
}

impl Bundle {
    /// Parses the vendored bundle nv-redfish ships — Redfish and Swordfish
    /// both, because DMTF types reference Swordfish ones (`Volume` names
    /// `FeaturesRegistry`).
    ///
    /// # Errors
    ///
    /// Returns [`IndexError`] naming the file that failed to read or parse.
    pub fn dmtf() -> Result<Self, IndexError> {
        let mut documents = Vec::new();
        let paths = nv_redfish_schema::glob_redfish_xml()
            .into_iter()
            .chain(nv_redfish_schema::glob_swordfish_xml());
        for path in paths {
            let content =
                fs::read_to_string(&path).map_err(|error| IndexError::Io(path.clone(), error))?;
            let document = Edmx::parse(&content)
                .map_err(|error| IndexError::Parse(path.clone(), format!("{error:?}")))?;
            documents.push(document);
        }
        Ok(Self {
            bundle: SchemaBundle {
                edmx_docs: documents,
                root_set_threshold: None,
            },
        })
    }

    /// Builds the queryable index over the parsed documents.
    ///
    /// # Errors
    ///
    /// Returns [`IndexError::Build`] if the bundle does not compile.
    ///
    /// # Panics
    ///
    /// Only if the compile thread cannot be spawned or itself panics.
    pub fn index(&self) -> Result<RedfishIndex<'_>, IndexError> {
        // The compiler's descent is deep enough to overflow a default test
        // stack in debug builds; nv-redfish's own build runs it the same
        // way. Scoped so the query can borrow the bundle.
        let (query, fold) = std::thread::scope(|scope| {
            std::thread::Builder::new()
                .stack_size(64 * 1024 * 1024)
                .spawn_scoped(scope, || {
                    let query = SchemaQuery::build(&self.bundle)
                        .map_err(|error| IndexError::Build(format!("{error:?}")))?;
                    let fold = Fold::build(&self.bundle)
                        .map_err(|error| IndexError::Build(format!("{error:?}")))?;
                    Ok::<_, IndexError>((query, fold))
                })
                .expect("the compile thread spawns")
                .join()
                .expect("the compile thread does not panic")
        })?;
        Ok(RedfishIndex { query, fold })
    }
}

/// Dotted source paths resolve to typed, nullability-aware fields.
pub struct RedfishIndex<'a> {
    query: SchemaQuery<'a>,
    fold: Fold<'a>,
}

impl fmt::Debug for RedfishIndex<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RedfishIndex")
    }
}

impl RedfishIndex<'_> {
    #[must_use]
    pub fn has_type(&self, name: &str) -> bool {
        self.query.has_entity(name)
    }

    /// Resolves a dotted path such as `Thresholds.UpperCritical.Reading`
    /// against the entity type named by `source_type`.
    #[must_use]
    pub fn resolve(&self, source_type: &str, path: &str) -> Option<ResolvedField> {
        let property = self.query.resolve(source_type, path)?;
        Some(ResolvedField {
            type_name: property.type_name.to_string(),
            class: property.class,
            nullable: property.nullable,
            collection: property.collection,
            enum_members: property.enum_members.map(|members| {
                members
                    .iter()
                    .map(|member| member.inner().to_string())
                    .collect()
            }),
        })
    }

    /// The namespace of the entity the fold picked for `name` — `Sensor`
    /// for `Sensor` — which spells the module its generated type lives in.
    #[must_use]
    pub fn entity_namespace(&self, name: &str) -> Option<String> {
        self.fold
            .entities
            .get(name)
            .map(|qname| qname.namespace.to_string())
    }

    /// Resolves a path into per-segment emission facts, cross-checked
    /// against [`resolve`](Self::resolve) so the two views cannot disagree
    /// about anything the query states.
    ///
    /// The boundary of that guarantee: type, class, nullability,
    /// cardinality, and enum members are compared fact by fact; the
    /// fold-only facts — base hops, `required`, a type definition's
    /// underlying primitive, and the entity pick itself — have no query
    /// counterpart to compare against, so a divergence there surfaces as a
    /// rustc error in the generated tree rather than here. Collapsing that
    /// residue means upstreaming these facts into `query.rs`; PLAN.md
    /// carries the follow-up.
    ///
    /// # Errors
    ///
    /// A message naming the path when it does not resolve — the lint runs
    /// first, so that is an emitter bug — or naming each fact the fold and
    /// the query disagree about, which is index drift and must stop the
    /// build.
    pub fn steps(&self, source_type: &str, path: &str) -> Result<Vec<Step>, String> {
        let steps = self
            .fold
            .steps(source_type, path)
            .ok_or_else(|| format!("`{source_type}.{path}` does not resolve in the fold"))?;
        let Some(leaf) = steps.last() else {
            return Err(format!("`{source_type}.{path}` resolved to no segments"));
        };
        let resolved = self
            .resolve(source_type, path)
            .ok_or_else(|| format!("`{source_type}.{path}` does not resolve in the query"))?;

        let mut diverged = Vec::new();
        let fold_type = format!("{}.{}", leaf.namespace, leaf.name);
        if fold_type != resolved.type_name {
            diverged.push(format!(
                "type (fold `{fold_type}`, query `{}`)",
                resolved.type_name
            ));
        }
        if leaf.class != resolved.class {
            diverged.push(format!(
                "class (fold {:?}, query {:?})",
                leaf.class, resolved.class
            ));
        }
        if leaf.nullable != resolved.nullable {
            diverged.push(format!(
                "nullability (fold {}, query {})",
                leaf.nullable, resolved.nullable
            ));
        }
        if leaf.collection != resolved.collection {
            diverged.push(format!(
                "cardinality (fold collection={}, query collection={})",
                leaf.collection, resolved.collection
            ));
        }
        if leaf.enum_members != resolved.enum_members {
            diverged.push("enum members".to_owned());
        }
        if !diverged.is_empty() {
            return Err(format!(
                "the fold and the query disagree about `{source_type}.{path}`: {}; \
                 the two run the same compile, so this is an index bug",
                diverged.join(", ")
            ));
        }
        Ok(steps)
    }
}

/// The compiled fold held directly: the same `compile_all` + optimizer run
/// the query performs, walked segment by segment for emission facts.
struct Fold<'a> {
    compiled: Compiled<'a>,
    entities: HashMap<&'a str, QualifiedName<'a>>,
}

impl<'a> Fold<'a> {
    fn build(
        bundle: &'a SchemaBundle,
    ) -> Result<Self, nv_redfish_csdl_compiler::compiler::Error<'a>> {
        let config = Config {
            entity_type_filter: EntityTypeFilter::new_restrictive(Vec::new()),
            ..Config::default()
        };
        let compiled = bundle.compile_all(config)?;
        let compiled = optimize(compiled, &OptimizerConfig::default());

        // The deterministic entity pick, mirroring `query.rs`'s `Rank`
        // exactly: schema family rooted at the short name first, then the
        // highest numerically-parsed version, then name order. `steps`
        // cross-checks every leaf against the query, so a divergence here
        // fails the build instead of emitting against the wrong type.
        let mut entities: HashMap<&str, QualifiedName<'_>> = HashMap::new();
        for qname in compiled.entity_types.keys() {
            let name = qname.name.inner().as_str();
            let replace = entities
                .get(name)
                .is_none_or(|current| Rank::new(*qname) > Rank::new(*current));
            if replace {
                entities.insert(name, *qname);
            }
        }
        Ok(Self { compiled, entities })
    }

    fn steps(&self, entity: &str, path: &str) -> Option<Vec<Step>> {
        let mut current = TypeRef::Entity(*self.entities.get(entity)?);
        let mut steps = Vec::new();
        let mut segments = path.split('.').peekable();
        while let Some(segment) = segments.next() {
            let (property, hops) = self.property_of(current, segment)?;
            let ((info, qname), collection) = match &property.ptype {
                OneOrCollection::One(inner) => (inner, false),
                OneOrCollection::Collection(inner) => (inner, true),
            };
            let (class, qname) = (info.class, *qname);
            let enum_members = match class {
                TypeClass::EnumType => self.compiled.enum_types.get(&qname).map(|declared| {
                    declared
                        .members
                        .iter()
                        .map(|member| member.name.inner().to_string())
                        .collect()
                }),
                _ => None,
            };
            let underlying = match class {
                TypeClass::TypeDefinition => self
                    .compiled
                    .type_definitions
                    .get(&qname)
                    .map(|definition| definition.underlying_type.name.inner().clone()),
                _ => None,
            };
            steps.push(Step {
                property: segment.to_owned(),
                hops,
                required: property.redfish.is_required.into_inner(),
                nullable: property.nullable.into_inner(),
                collection,
                class,
                namespace: qname.namespace.to_string(),
                name: qname.name.inner().clone(),
                underlying,
                enum_members,
            });
            if segments.peek().is_none() {
                return Some(steps);
            }
            // A collection element is not addressable: descending one would
            // emit field access on a `Vec`. The lint refuses such paths
            // first; this keeps emission honest if it ever misses one.
            if class != TypeClass::ComplexType || collection {
                return None;
            }
            current = TypeRef::Complex(qname);
        }
        None
    }

    /// The named structural property with the number of base-chain hops
    /// that reached it.
    fn property_of(&self, mut tref: TypeRef<'a>, name: &str) -> Option<(&Property<'a>, usize)> {
        let mut hops = 0;
        while let Some((properties, base)) = self.declaration(tref) {
            if let Some(property) = properties
                .properties
                .iter()
                .find(|property| property.name.inner().inner() == name)
            {
                return Some((property, hops));
            }
            hops += 1;
            tref = match tref {
                TypeRef::Entity(_) => TypeRef::Entity(base?),
                TypeRef::Complex(_) => TypeRef::Complex(base?),
            };
        }
        None
    }

    fn declaration(
        &self,
        tref: TypeRef<'a>,
    ) -> Option<(&Properties<'a>, Option<QualifiedName<'a>>)> {
        match tref {
            TypeRef::Entity(qname) => self
                .compiled
                .entity_types
                .get(&qname)
                .map(|entity| (&entity.properties, entity.base)),
            TypeRef::Complex(qname) => self
                .compiled
                .complex_types
                .get(&qname)
                .map(|complex| (&complex.properties, complex.base)),
        }
    }
}

#[derive(Clone, Copy)]
enum TypeRef<'a> {
    Entity(QualifiedName<'a>),
    Complex(QualifiedName<'a>),
}

/// Mirrors `query.rs`'s `Rank`; see the comment in [`Fold::build`].
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Rank<'a> {
    own_family: bool,
    version: Option<Version>,
    qname: QualifiedName<'a>,
}

impl<'a> Rank<'a> {
    fn new(qname: QualifiedName<'a>) -> Self {
        Self {
            own_family: qname.namespace.get_id(0) == Some(qname.name),
            version: qname
                .namespace
                .get_id(1)
                .and_then(|id| Version::parse(id.inner().as_str())),
            qname,
        }
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl Version {
    fn parse(id: &str) -> Option<Self> {
        let mut parts = id.strip_prefix('v')?.split('_');
        let version = Self {
            major: parts.next()?.parse().ok()?,
            minor: parts.next()?.parse().ok()?,
            patch: parts.next()?.parse().ok()?,
        };
        parts.next().is_none().then_some(version)
    }
}

#[cfg(test)]
mod tests {
    use super::Bundle;
    use super::Shape;
    use super::TypeClass;

    #[test]
    fn the_worked_examples_paths_resolve_with_their_types() {
        let bundle = Bundle::dmtf().expect("the vendored bundle parses");
        let index = bundle.index().expect("the bundle compiles");

        assert!(index.has_type("Sensor"));
        assert!(!index.has_type("Sensr"));

        // The sample's anchor: a nullable decimal, because a sensor that
        // cannot answer reports null.
        let reading = index
            .resolve("Sensor", "Reading")
            .expect("Reading resolves");
        assert_eq!(reading.type_name, "Edm.Decimal");
        assert!(reading.nullable);
        assert!(!reading.collection);
        assert!(reading.enum_members.is_none());

        // A base-chain property: Id lives on Resource, not Sensor. Its
        // typedef is still a scalar; a complex type is not.
        let id = index
            .resolve("Sensor", "Id")
            .expect("Id resolves via the base chain");
        assert_eq!(id.type_name, "Resource.Id");
        assert!(id.is_scalar());
        let status = index.resolve("Sensor", "Status").expect("Status resolves");
        assert!(!status.is_scalar());

        // Through complex types, with the closed vocabularies attached.
        let health = index
            .resolve("Sensor", "Status.Health")
            .expect("Status.Health resolves");
        let members = health.enum_members.expect("Health is an enum");
        assert!(members.iter().any(|member| member == "OK"));

        let activation = index
            .resolve("Sensor", "Thresholds.UpperCritical.Activation")
            .expect("the threshold activation resolves");
        let members = activation.enum_members.expect("Activation is an enum");
        assert!(members.iter().any(|member| member == "Disabled"));

        let kind = index
            .resolve("Sensor", "ReadingType")
            .expect("ReadingType resolves");
        let members = kind.enum_members.expect("ReadingType is an enum");
        assert!(members.iter().any(|member| member == "AirFlowCMM"));

        // Unknown paths and descent through scalars resolve to nothing.
        assert!(index.resolve("Sensor", "ReadingTypo").is_none());
        assert!(index.resolve("Sensor", "Reading.Deeper").is_none());
    }

    #[test]
    fn steps_carry_what_field_access_generation_needs() {
        let bundle = Bundle::dmtf().expect("the vendored bundle parses");
        let index = bundle.index().expect("the bundle compiles");

        // Id: one base hop up the collapsed chain, required and
        // non-nullable — generation makes it a bare field — and a typedef
        // whose underlying primitive conversion selection keys on.
        let steps = index.steps("Sensor", "Id").expect("Id resolves");
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].hops, 1);
        assert_eq!(steps[0].shape(), Shape::Bare);
        assert_eq!(steps[0].class, TypeClass::TypeDefinition);
        assert_eq!(steps[0].underlying.as_deref(), Some("String"));

        // A nullable leaf under two presence-only complex segments, each
        // declared on its own type: no hops, single-`Option` shapes.
        let steps = index
            .steps("Sensor", "Thresholds.UpperCritical.Reading")
            .expect("the threshold reading resolves");
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].shape(), Shape::Optional);
        assert_eq!(steps[0].hops, 0);
        assert_eq!(steps[1].shape(), Shape::Optional);
        assert_eq!(steps[1].hops, 0);
        assert_eq!(steps[2].shape(), Shape::Nullable);
        assert_eq!(steps[2].namespace, "Edm");
        assert_eq!(steps[2].name, "Decimal");

        // Nullability can live above a non-nullable leaf. A path-wide null
        // policy check must therefore inspect every step, not only the query's
        // leaf result.
        let steps = index
            .steps("Chassis", "Doors.Front.UserLabel")
            .expect("the nested door label resolves");
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].shape(), Shape::Optional);
        assert_eq!(steps[1].shape(), Shape::Nullable);
        assert_eq!(steps[2].shape(), Shape::Optional);

        // The fold refuses what the query refuses.
        assert!(index.steps("Sensor", "ReadingTypo").is_err());
    }
}
