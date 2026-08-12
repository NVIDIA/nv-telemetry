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
use std::ops::Deref;
use std::path::Component;
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
    /// A discovered manifest did not resolve to a normal path below the
    /// selected workspace root.
    Path(WorkspaceRelativePathError),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(path, error) => write!(f, "{}: {error}", path.display()),
            Self::Parse(path, error) => write!(f, "{}: {error}", path.display()),
            Self::SchemaMissing => {
                write!(f, "the descriptor set does not define `{MANIFEST}`")
            }
            Self::Path(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ManifestError {}

/// Why a path cannot identify a manifest inside the selected workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceRelativePathError {
    path: PathBuf,
    detail: &'static str,
}

impl fmt::Display for WorkspaceRelativePathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "manifest path `{}` is not workspace-relative: {}",
            self.path.display(),
            self.detail
        )
    }
}

impl std::error::Error for WorkspaceRelativePathError {}

/// A non-empty, normalized path below the selected workspace root.
///
/// This is the compiler's trust boundary for manifest identity. Diagnostics,
/// provenance, and generated headers may render this value without leaking a
/// checkout-specific absolute path or accepting a parent traversal.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkspaceRelativePath(PathBuf);

impl WorkspaceRelativePath {
    /// Validates a path supplied by a loader or a programmatic compiler user.
    ///
    /// # Errors
    ///
    /// Empty, absolute, parent-relative, and non-normal paths are rejected.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, WorkspaceRelativePathError> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(WorkspaceRelativePathError {
                path,
                detail: "the path is empty",
            });
        }
        if !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        {
            return Err(WorkspaceRelativePathError {
                path,
                detail: "only normal relative components are allowed",
            });
        }
        Ok(Self(path))
    }

    /// Renders the path with stable `/` separators for generated text.
    #[must_use]
    pub fn rendered(&self) -> String {
        self.0
            .iter()
            .map(|component| component.to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }
}

impl AsRef<Path> for WorkspaceRelativePath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Deref for WorkspaceRelativePath {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Display for WorkspaceRelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.rendered())
    }
}

/// One loaded manifest. Enum fields carry raw numbers; the lint interprets
/// them.
#[derive(Clone, Debug)]
pub struct ManifestSpec {
    /// Validated workspace-relative manifest path.
    pub path: WorkspaceRelativePath,
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

impl ManifestSpec {
    /// The manifest's path from the workspace root, for diagnostics and
    /// generated headers.
    #[must_use]
    pub fn relative_path(&self) -> String {
        self.path.rendered()
    }
}

impl ProjectionSpec {
    /// What this projection means once its expansion is applied: one
    /// instance per member — the shared declarations plus the expansion's,
    /// placeholders substituted — or the projection itself when it declares
    /// no expansion. The lint checks these instances and emission generates
    /// from them, so the two cannot disagree about what an expansion means.
    #[must_use]
    pub fn instances(&self) -> Vec<Self> {
        let Some(expansion) = &self.expansion else {
            return vec![self.clone()];
        };
        expansion
            .members
            .iter()
            .map(|member| {
                let mut instance = self.clone();
                instance.expansion = None;
                for field in &expansion.fields {
                    let mut field = field.clone();
                    field.source_path = substitute(&field.source_path, member);
                    instance.fields.push(field);
                }
                for assembly in &expansion.map_assemblies {
                    let mut assembly = assembly.clone();
                    for entry in &mut assembly.entries {
                        entry.source_path = substitute(&entry.source_path, member);
                    }
                    instance.map_assemblies.push(assembly);
                }
                for constant in &expansion.constants {
                    let mut constant = constant.clone();
                    constant.value = substitute(&constant.value, member);
                    instance.constants.push(constant);
                }
                instance
            })
            .collect()
    }
}

/// Substitutes the expansion placeholders for one member. A brace left in
/// the result is a placeholder that failed; the lint reports it and
/// emission never sees one.
#[must_use]
pub fn substitute(text: &str, member: &str) -> String {
    use heck::ToKebabCase as _;
    text.replace("{member-kebab}", &member.to_kebab_case())
        .replace("{member}", member)
}

#[derive(Clone, Debug)]
pub struct ExpansionSpec {
    pub members: Vec<String>,
    pub fields: Vec<FieldSpec>,
    pub constants: Vec<ConstantSpec>,
    pub map_assemblies: Vec<AssemblySpec>,
}

// Compared by value: emission derives one subject per source type and must
// recognize two projections declaring the same identity as agreeing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubjectSpec {
    pub kind: String,
    pub scope: Vec<ScopeSpec>,
    pub id_path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
    let canonical_root =
        fs::canonicalize(root).map_err(|error| ManifestError::Io(root.to_path_buf(), error))?;

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
        ensure_workspace_member(&canonical_root, &path)?;
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
        let relative = workspace_relative(root, &path)?;
        manifests.push(manifest_spec(&message, relative, crate_source));
    }
    Ok(manifests)
}

fn ensure_workspace_member(canonical_root: &Path, path: &Path) -> Result<(), ManifestError> {
    let canonical_path =
        fs::canonicalize(path).map_err(|error| ManifestError::Io(path.to_path_buf(), error))?;
    if canonical_path.starts_with(canonical_root) {
        return Ok(());
    }
    Err(ManifestError::Path(WorkspaceRelativePathError {
        path: path.to_path_buf(),
        detail: "the path resolves outside the selected workspace root",
    }))
}

fn workspace_relative(root: &Path, path: &Path) -> Result<WorkspaceRelativePath, ManifestError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        ManifestError::Path(WorkspaceRelativePathError {
            path: path.to_path_buf(),
            detail: "the path is outside the selected workspace root",
        })
    })?;
    WorkspaceRelativePath::new(relative).map_err(ManifestError::Path)
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

fn manifest_spec(
    message: &DynamicMessage,
    path: WorkspaceRelativePath,
    crate_source: String,
) -> ManifestSpec {
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

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::path::PathBuf;

    use super::ensure_workspace_member;
    use super::workspace_relative;
    use super::ManifestSpec;
    use super::WorkspaceRelativePath;

    #[test]
    fn manifest_paths_are_relative_to_the_workspace_not_an_ancestor_named_sources() {
        let root = PathBuf::from("sources/checkout/nv-telemetry");
        let absolute = root.join("sources/redfish/manifests/sensor.textpb");
        let path = workspace_relative(&root, &absolute).expect("the path is below the workspace");
        let manifest = ManifestSpec {
            path,
            crate_source: "redfish".to_owned(),
            source: "redfish".to_owned(),
            backend: 1,
            index: "nv-redfish-schema/dmtf".to_owned(),
            projections: Vec::new(),
            subject: None,
        };

        assert_eq!(
            manifest.relative_path(),
            "sources/redfish/manifests/sensor.textpb"
        );
    }

    #[test]
    fn manifest_paths_reject_every_workspace_escape_shape() {
        for path in [
            PathBuf::new(),
            PathBuf::from("/tmp/sensor.textpb"),
            PathBuf::from("../sensor.textpb"),
            PathBuf::from("sources/../sensor.textpb"),
            PathBuf::from("./sources/redfish/sensor.textpb"),
        ] {
            assert!(
                WorkspaceRelativePath::new(path.clone()).is_err(),
                "{path:?} must not cross the compiler boundary"
            );
        }
    }

    #[test]
    fn workspace_relative_refuses_a_path_below_another_root() {
        let error = workspace_relative(
            Path::new("checkout-a"),
            Path::new("checkout-b/sources/redfish/manifests/sensor.textpb"),
        )
        .expect_err("a different checkout cannot become provenance");

        assert!(error.to_string().contains("outside the selected workspace"));
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_manifest_cannot_resolve_outside_the_workspace() {
        use std::fs;
        use std::os::unix::fs::symlink;
        use std::time::SystemTime;

        let serial = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "nv-telemetry-manifest-root-{}-{serial}",
            std::process::id(),
        ));
        let external = std::env::temp_dir().join(format!(
            "nv-telemetry-manifest-external-{}-{serial}",
            std::process::id(),
        ));
        fs::create_dir_all(&root).expect("the workspace fixture is created");
        fs::create_dir_all(&external).expect("the external fixture is created");
        let external_manifest = external.join("sensor.textpb");
        fs::write(&external_manifest, "source: \"redfish\"\n")
            .expect("the external manifest is written");
        let linked = root.join("sensor.textpb");
        symlink(&external_manifest, &linked).expect("the manifest is linked");
        let canonical_root = fs::canonicalize(&root).expect("the fixture root resolves");

        let error = ensure_workspace_member(&canonical_root, &linked)
            .expect_err("external manifest content cannot cross the loader boundary");
        assert!(error.to_string().contains("resolves outside"));

        fs::remove_dir_all(&root).expect("the workspace fixture is removed");
        fs::remove_dir_all(&external).expect("the external fixture is removed");
    }
}
