// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The Redfish backend's schema index: a thin view over nv-redfish's
//! `SchemaQuery`.

use std::fmt;
use std::fs;

use nv_redfish_csdl_compiler::compiler::SchemaBundle;
use nv_redfish_csdl_compiler::compiler::TypeClass;
use nv_redfish_csdl_compiler::edmx::Edmx;
use nv_redfish_csdl_compiler::query::SchemaQuery;

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
        let query = std::thread::scope(|scope| {
            std::thread::Builder::new()
                .stack_size(64 * 1024 * 1024)
                .spawn_scoped(scope, || SchemaQuery::build(&self.bundle))
                .expect("the compile thread spawns")
                .join()
                .expect("the compile thread does not panic")
        })
        .map_err(|error| IndexError::Build(format!("{error:?}")))?;
        Ok(RedfishIndex { query })
    }
}

/// Dotted source paths resolve to typed, nullability-aware fields.
pub struct RedfishIndex<'a> {
    query: SchemaQuery<'a>,
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
}

#[cfg(test)]
mod tests {
    use super::Bundle;

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
}
