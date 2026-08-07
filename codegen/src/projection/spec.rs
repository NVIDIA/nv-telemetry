// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Manifest loading: text-format instances of `nv.telemetry.mapping.v1`
//! become typed specs. The parser rejects unknown fields against the
//! shipped descriptor set, so a manifest that loads is shaped like the
//! schema; whether it means anything is the lint's question. Field
//! semantics are documented on `manifest.proto`, which these mirror.

// Public exhaustive fields on purpose: the specs mirror the schema and
// tests construct them literally.
#![allow(clippy::exhaustive_structs, clippy::exhaustive_enums)]

use std::fmt;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use prost_reflect::DescriptorPool;
use prost_reflect::DynamicMessage;
use prost_reflect::Value;

/// Full name of the manifest message in the shipped descriptor set.
const MANIFEST: &str = "nv.telemetry.mapping.v1.Manifest";

/// Why manifests could not be loaded.
#[derive(Debug)]
pub enum ManifestError {
    /// The manifest directory or a file in it could not be read.
    Io(PathBuf, std::io::Error),
    /// A manifest is not valid text format for the mapping schema.
    Parse(PathBuf, String),
    /// A build inconsistency, not a manifest author's fault.
    SchemaMissing,
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(path, error) => write!(f, "{}: {error}", path.display()),
            Self::Parse(path, error) => write!(f, "{}: {error}", path.display()),
            Self::SchemaMissing => {
                write!(f, "the descriptor set does not define `{MANIFEST}`")
            }
        }
    }
}

impl std::error::Error for ManifestError {}

/// One loaded manifest. Enum fields carry raw numbers; the lint interprets
/// them.
#[derive(Clone, Debug)]
pub struct ManifestSpec {
    pub path: PathBuf,
    /// The `sources/` directory the manifest was found under.
    pub crate_source: String,
    pub source: String,
    pub backend: i32,
    pub index: String,
    pub projections: Vec<ProjectionSpec>,
    pub subject: Option<SubjectSpec>,
}

#[derive(Clone, Debug)]
pub struct ProjectionSpec {
    pub name: String,
    pub source_type: String,
    pub target_type: String,
    pub subject: Option<SubjectSpec>,
    pub fields: Vec<FieldSpec>,
    pub iterate: String,
    pub versions: usize,
    pub constants: Vec<ConstantSpec>,
    pub map_assemblies: Vec<AssemblySpec>,
    pub expansion: Option<ExpansionSpec>,
}

#[derive(Clone, Debug)]
pub struct ExpansionSpec {
    pub members: Vec<String>,
    pub fields: Vec<FieldSpec>,
    pub constants: Vec<ConstantSpec>,
    pub map_assemblies: Vec<AssemblySpec>,
}

#[derive(Clone, Debug)]
pub struct SubjectSpec {
    pub kind: String,
    pub scope: Vec<ScopeSpec>,
    pub id_path: String,
}

#[derive(Clone, Debug)]
pub enum ScopeSpec {
    PayloadPath(String),
    LocationTemplate {
        template: String,
        capture: String,
    },
    PathKey(String),
    /// No case set; the lint rejects it.
    Unset,
}

#[derive(Clone, Debug)]
pub struct FieldSpec {
    pub source_path: String,
    pub target_field: String,
    pub required: bool,
    pub anchor: bool,
    pub unit: String,
    pub unit_path: String,
    pub known_values: Vec<String>,
    pub null_policy: i32,
    pub cardinality: i32,
    pub value_map: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct ConstantSpec {
    pub target_field: String,
    pub value: String,
}

#[derive(Clone, Debug)]
pub struct AssemblySpec {
    pub target_field: String,
    pub entries: Vec<EntrySpec>,
}

#[derive(Clone, Debug)]
pub struct EntrySpec {
    pub key: String,
    pub source_path: String,
    pub null_policy: i32,
    pub value_map: Vec<(String, String)>,
}

/// Loads every manifest under `sources/*/manifests/*.textpb`, sorted by path
/// so diagnostics and later emission are deterministic.
///
/// # Errors
///
/// Returns the first file that cannot be read or does not parse as the
/// mapping schema.
pub fn load(root: &Path, pool: &DescriptorPool) -> Result<Vec<ManifestSpec>, ManifestError> {
    let descriptor = pool
        .get_message_by_name(MANIFEST)
        .ok_or(ManifestError::SchemaMissing)?;

    let sources = root.join("sources");
    let mut files = Vec::new();
    let entries = match fs::read_dir(&sources) {
        Ok(entries) => entries,
        // A tree without protocol crates has no manifests to load.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(ManifestError::Io(sources, error)),
    };
    for entry in entries {
        let entry = entry.map_err(|error| ManifestError::Io(sources.clone(), error))?;
        let manifests = entry.path().join("manifests");
        let candidates = match fs::read_dir(&manifests) {
            Ok(candidates) => candidates,
            // No manifests directory is a crate without declarations; any
            // other failure would silently skip checking, which is the one
            // thing this loader must not do.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(ManifestError::Io(manifests, error)),
        };
        for candidate in candidates {
            let candidate =
                candidate.map_err(|error| ManifestError::Io(manifests.clone(), error))?;
            let path = candidate.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "textpb")
            {
                files.push(path);
            }
        }
    }
    files.sort();

    let mut manifests = Vec::new();
    for path in files {
        let text =
            fs::read_to_string(&path).map_err(|error| ManifestError::Io(path.clone(), error))?;
        let message = DynamicMessage::parse_text_format(descriptor.clone(), &text)
            .map_err(|error| ManifestError::Parse(path.clone(), error.to_string()))?;
        let crate_source = path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        manifests.push(manifest_spec(&message, path, crate_source));
    }
    Ok(manifests)
}

// The readers default missing fields exactly as proto3 does.

fn string(message: &DynamicMessage, field: &str) -> String {
    message
        .get_field_by_name(field)
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_default()
}

fn boolean(message: &DynamicMessage, field: &str) -> bool {
    message
        .get_field_by_name(field)
        .and_then(|value| value.as_bool())
        .unwrap_or_default()
}

fn number(message: &DynamicMessage, field: &str) -> i32 {
    message
        .get_field_by_name(field)
        .and_then(|value| value.as_enum_number())
        .unwrap_or_default()
}

fn strings(message: &DynamicMessage, field: &str) -> Vec<String> {
    list(message, field, |value| {
        value.as_str().map(ToOwned::to_owned)
    })
}

fn messages<T>(
    message: &DynamicMessage,
    field: &str,
    read: impl Fn(&DynamicMessage) -> T,
) -> Vec<T> {
    list(message, field, |value| value.as_message().map(&read))
}

/// A singular message field, `None` when unset — `get_field_by_name` alone
/// would hand back the type's default.
fn optional<T>(
    message: &DynamicMessage,
    field: &str,
    read: impl Fn(&DynamicMessage) -> T,
) -> Option<T> {
    message
        .get_field_by_name(field)
        .and_then(|value| value.as_message().map(read))
        .filter(|_| message.has_field_by_name(field))
}

fn list<T>(
    message: &DynamicMessage,
    field: &str,
    mut read: impl FnMut(&Value) -> Option<T>,
) -> Vec<T> {
    message
        .get_field_by_name(field)
        .and_then(|value| {
            value
                .as_list()
                .map(|entries| entries.iter().filter_map(&mut read).collect())
        })
        .unwrap_or_default()
}

fn manifest_spec(message: &DynamicMessage, path: PathBuf, crate_source: String) -> ManifestSpec {
    ManifestSpec {
        path,
        crate_source,
        source: string(message, "source"),
        backend: number(message, "backend"),
        index: string(message, "index"),
        projections: messages(message, "projections", projection_spec),
        subject: optional(message, "subject", subject_spec),
    }
}

fn projection_spec(message: &DynamicMessage) -> ProjectionSpec {
    ProjectionSpec {
        name: string(message, "name"),
        source_type: string(message, "source_type"),
        target_type: string(message, "target_type"),
        subject: optional(message, "subject", subject_spec),
        fields: messages(message, "fields", field_spec),
        iterate: string(message, "iterate"),
        versions: list(message, "versions", |value| value.as_message().map(|_| ())).len(),
        constants: messages(message, "constants", constant_spec),
        map_assemblies: messages(message, "map_assemblies", assembly_spec),
        expansion: optional(message, "expansion", expansion_spec),
    }
}

fn constant_spec(message: &DynamicMessage) -> ConstantSpec {
    ConstantSpec {
        target_field: string(message, "target_field"),
        value: string(message, "value"),
    }
}

fn expansion_spec(message: &DynamicMessage) -> ExpansionSpec {
    ExpansionSpec {
        members: strings(message, "members"),
        fields: messages(message, "fields", field_spec),
        constants: messages(message, "constants", constant_spec),
        map_assemblies: messages(message, "map_assemblies", assembly_spec),
    }
}

fn subject_spec(message: &DynamicMessage) -> SubjectSpec {
    SubjectSpec {
        kind: string(message, "kind"),
        scope: messages(message, "scope", scope_spec),
        id_path: string(message, "id_path"),
    }
}

fn scope_spec(message: &DynamicMessage) -> ScopeSpec {
    if message.has_field_by_name("payload_path") {
        return ScopeSpec::PayloadPath(string(message, "payload_path"));
    }
    if message.has_field_by_name("location_template") {
        return ScopeSpec::LocationTemplate {
            template: string(message, "location_template"),
            capture: string(message, "capture"),
        };
    }
    if message.has_field_by_name("path_key") {
        return ScopeSpec::PathKey(string(message, "path_key"));
    }
    ScopeSpec::Unset
}

fn field_spec(message: &DynamicMessage) -> FieldSpec {
    FieldSpec {
        source_path: string(message, "source_path"),
        target_field: string(message, "target_field"),
        required: boolean(message, "required"),
        anchor: boolean(message, "anchor"),
        unit: string(message, "unit"),
        unit_path: string(message, "unit_path"),
        known_values: strings(message, "known_values"),
        null_policy: number(message, "null_policy"),
        cardinality: number(message, "cardinality"),
        value_map: value_map(message),
    }
}

fn assembly_spec(message: &DynamicMessage) -> AssemblySpec {
    AssemblySpec {
        target_field: string(message, "target_field"),
        entries: messages(message, "entries", |entry| EntrySpec {
            key: string(entry, "key"),
            source_path: string(entry, "source_path"),
            null_policy: number(entry, "null_policy"),
            value_map: value_map(entry),
        }),
    }
}

fn value_map(message: &DynamicMessage) -> Vec<(String, String)> {
    messages(message, "value_map", |mapping| {
        (string(mapping, "from"), string(mapping, "to"))
    })
}
