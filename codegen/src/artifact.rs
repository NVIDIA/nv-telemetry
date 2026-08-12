// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Filesystem boundary for generated artifacts.
//!
//! Compilation decides bytes and workspace-relative destinations. This module
//! is the only place that writes them, discovers stale projection files, or
//! removes them, and it refuses paths whose existing components traverse
//! symlinks or whose lexical form leaves the selected workspace.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use crate::Mode;

/// A filesystem failure paired with the exact generated-tree path involved.
#[derive(Debug)]
pub(crate) struct ArtifactIo {
    pub(crate) path: PathBuf,
    pub(crate) error: io::Error,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct OwnedArtifacts {
    pub(crate) stale: Vec<PathBuf>,
    pub(crate) removed: Vec<PathBuf>,
    pub(crate) unrecognized: Vec<PathBuf>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ExpectedArtifacts {
    pub(crate) written: Vec<PathBuf>,
    pub(crate) stale: Vec<PathBuf>,
}

#[derive(Debug)]
pub(crate) struct ExpectedArtifactIo {
    pub(crate) failure: ArtifactIo,
    pub(crate) written: Vec<PathBuf>,
}

struct ReconcileContext<'a> {
    root: &'a Path,
    canonical_root: &'a Path,
    expected: &'a BTreeSet<PathBuf>,
    expected_directories: &'a BTreeSet<PathBuf>,
    mode: Mode,
}

/// Validates every destination before generation performs its first write.
pub(crate) fn validate_output_paths(
    root: &Path,
    paths: &BTreeSet<PathBuf>,
) -> Result<(), ArtifactIo> {
    let canonical_root = fs::canonicalize(root).map_err(|error| ArtifactIo {
        path: root.to_path_buf(),
        error,
    })?;
    for path in paths {
        validate_workspace_path(root, &canonical_root, path)?;
    }
    Ok(())
}

/// Reconciles the compiler's explicit output set after validating the whole
/// set and before orphan discovery begins.
pub(crate) fn reconcile_expected_artifacts(
    root: &Path,
    artifacts: Vec<(PathBuf, String)>,
    mode: Mode,
) -> Result<ExpectedArtifacts, ExpectedArtifactIo> {
    let paths = artifacts
        .iter()
        .map(|(path, _)| path.clone())
        .collect::<BTreeSet<_>>();
    validate_output_paths(root, &paths).map_err(|failure| ExpectedArtifactIo {
        failure,
        written: Vec::new(),
    })?;

    let mut outcome = ExpectedArtifacts::default();
    for (path, rendered) in artifacts {
        let committed = match fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(expected_failure(path, error, &outcome)),
        };
        if committed.as_deref() == Some(rendered.as_str()) {
            continue;
        }

        match mode {
            Mode::Check => outcome.stale.push(path),
            Mode::Generate => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|error| expected_failure(parent.to_path_buf(), error, &outcome))?;
                }
                // Directory creation and prior writes may have changed which
                // components exist, so check the destination again at the
                // mutation point.
                validate_output_paths(root, &BTreeSet::from([path.clone()])).map_err(
                    |failure| ExpectedArtifactIo {
                        failure,
                        written: outcome.written.clone(),
                    },
                )?;
                fs::write(&path, &rendered)
                    .map_err(|error| expected_failure(path.clone(), error, &outcome))?;
                outcome.written.push(path);
            }
        }
    }
    Ok(outcome)
}

fn expected_failure(
    path: PathBuf,
    error: io::Error,
    outcome: &ExpectedArtifacts,
) -> ExpectedArtifactIo {
    ExpectedArtifactIo {
        failure: ArtifactIo { path, error },
        written: outcome.written.clone(),
    }
}

/// Reconciles projection directories as sets, not just expected file
/// contents. A file is removable only when its own header positively marks it
/// as this compiler's output; everything else is surfaced and preserved.
pub(crate) fn reconcile_projection_artifacts(
    root: &Path,
    expected: &BTreeSet<PathBuf>,
    mode: Mode,
) -> Result<OwnedArtifacts, ArtifactIo> {
    let canonical_root = fs::canonicalize(root).map_err(|error| ArtifactIo {
        path: root.to_path_buf(),
        error,
    })?;
    let sources = root.join("sources");
    let generated_directories = discover_generated_directories(root, &canonical_root, &sources)?;
    let expected_directories = expected
        .iter()
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .collect::<BTreeSet<_>>();
    let context = ReconcileContext {
        root,
        canonical_root: &canonical_root,
        expected,
        expected_directories: &expected_directories,
        mode,
    };
    let mut outcome = OwnedArtifacts::default();
    for directory in generated_directories {
        reconcile_directory(&context, &directory, &mut outcome)?;
    }
    Ok(outcome)
}

fn discover_generated_directories(
    root: &Path,
    canonical_root: &Path,
    sources: &Path,
) -> Result<Vec<PathBuf>, ArtifactIo> {
    let source_crates = match fs::read_dir(sources) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(ArtifactIo {
                path: sources.to_path_buf(),
                error,
            });
        }
    };
    validate_workspace_path(root, canonical_root, sources)?;

    let mut directories = Vec::new();
    for entry in source_crates {
        let entry = entry.map_err(|error| ArtifactIo {
            path: sources.to_path_buf(),
            error,
        })?;
        let file_type = entry.file_type().map_err(|error| ArtifactIo {
            path: entry.path(),
            error,
        })?;
        // A linked source crate is not owned by this workspace. Expected
        // outputs below it have already been rejected by validate_output_paths;
        // an unrelated linked tree is simply outside orphan discovery.
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }

        let generated = entry.path().join("src/generated");
        match fs::symlink_metadata(&generated) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(invalid_path(
                    generated,
                    "generated artifact directories must not be symbolic links",
                ));
            }
            Ok(metadata) if metadata.is_dir() => {
                validate_workspace_path(root, canonical_root, &generated)?;
                directories.push(generated);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(ArtifactIo {
                    path: generated,
                    error,
                });
            }
        }
    }
    directories.sort();
    Ok(directories)
}

fn reconcile_directory(
    context: &ReconcileContext<'_>,
    directory: &Path,
    outcome: &mut OwnedArtifacts,
) -> Result<(), ArtifactIo> {
    let entries = fs::read_dir(directory).map_err(|error| ArtifactIo {
        path: directory.to_path_buf(),
        error,
    })?;
    let mut discovered = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| ArtifactIo {
            path: directory.to_path_buf(),
            error,
        })?;
        let path = entry.path();
        if context.expected.contains(&path) {
            continue;
        }
        let file_type = entry.file_type().map_err(|error| ArtifactIo {
            path: path.clone(),
            error,
        })?;
        let generated = if file_type.is_file() && !file_type.is_symlink() {
            let text = fs::read_to_string(&path).map_err(|error| ArtifactIo {
                path: path.clone(),
                error,
            })?;
            has_projection_generator_header(&text)
        } else {
            false
        };
        discovered.push((path, generated));
    }
    discovered.sort_by(|left, right| left.0.cmp(&right.0));

    let owned = context.expected_directories.contains(directory)
        || discovered.iter().any(|(_, generated)| *generated);
    if !owned {
        return Ok(());
    }
    reconcile_discovered(context, discovered, outcome)
}

fn reconcile_discovered(
    context: &ReconcileContext<'_>,
    discovered: Vec<(PathBuf, bool)>,
    outcome: &mut OwnedArtifacts,
) -> Result<(), ArtifactIo> {
    for (path, generated) in discovered {
        if !generated {
            outcome.unrecognized.push(path);
            continue;
        }
        match context.mode {
            Mode::Check => outcome.stale.push(path),
            Mode::Generate => {
                // Revalidate immediately before the destructive operation.
                validate_workspace_path(context.root, context.canonical_root, &path)?;
                fs::remove_file(&path).map_err(|error| ArtifactIo {
                    path: path.clone(),
                    error,
                })?;
                outcome.removed.push(path);
            }
        }
    }
    Ok(())
}

fn validate_workspace_path(
    root: &Path,
    canonical_root: &Path,
    path: &Path,
) -> Result<(), ArtifactIo> {
    let relative = path.strip_prefix(root).map_err(|_| {
        invalid_path(
            path.to_path_buf(),
            "generated artifact path leaves the selected workspace",
        )
    })?;
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(invalid_path(
            path.to_path_buf(),
            "generated artifact paths contain only normal workspace-relative components",
        ));
    }

    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(invalid_path(
                    current,
                    "generated artifact paths must not traverse symbolic links",
                ));
            }
            Ok(_) => {
                let canonical = fs::canonicalize(&current).map_err(|error| ArtifactIo {
                    path: current.clone(),
                    error,
                })?;
                if !canonical.starts_with(canonical_root) {
                    return Err(invalid_path(
                        current,
                        "generated artifact path resolves outside the selected workspace",
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(ArtifactIo {
                    path: current,
                    error,
                });
            }
        }
    }
    Ok(())
}

fn invalid_path(path: PathBuf, detail: &'static str) -> ArtifactIo {
    ArtifactIo {
        path,
        error: io::Error::new(io::ErrorKind::InvalidInput, detail),
    }
}

fn has_projection_generator_header(text: &str) -> bool {
    let header = text.lines().take(16).collect::<Vec<_>>().join("\n");
    header.contains("//! Generated ")
        && header.contains("`make codegen`")
        && header.contains("Do not edit")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;

    use super::has_projection_generator_header;
    use super::reconcile_expected_artifacts;
    use super::reconcile_projection_artifacts;
    use super::validate_output_paths;
    use crate::Mode;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    fn fixture_root(label: &str) -> PathBuf {
        let serial = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "nv-telemetry-codegen-{label}-{}-{serial}",
            std::process::id()
        ))
    }

    #[test]
    fn generated_orphans_are_removed_but_unrecognized_files_are_preserved() {
        let root = fixture_root("artifacts");
        let generated = root.join("sources/example/src/generated");
        fs::create_dir_all(&generated).expect("the fixture directory is created");
        let orphan = generated.join("old.rs");
        fs::write(
            &orphan,
            "//! Generated by `make codegen`. Do not edit.\n\npub fn old() {}\n",
        )
        .expect("the generated fixture is written");
        let hand_written = generated.join("keep.rs");
        fs::write(&hand_written, "pub fn keep() {}\n")
            .expect("the hand-written fixture is written");

        let checked = reconcile_projection_artifacts(&root, &BTreeSet::new(), Mode::Check)
            .expect("the fixture can be inspected");
        assert_eq!(checked.stale, vec![orphan.clone()]);
        assert_eq!(checked.unrecognized, vec![hand_written.clone()]);
        assert!(orphan.exists());
        assert!(hand_written.exists());

        let generated_outcome =
            reconcile_projection_artifacts(&root, &BTreeSet::new(), Mode::Generate)
                .expect("the fixture can be reconciled");
        assert_eq!(generated_outcome.removed, vec![orphan.clone()]);
        assert_eq!(generated_outcome.unrecognized, vec![hand_written.clone()]);
        assert!(!orphan.exists());
        assert!(hand_written.exists());

        fs::remove_dir_all(&root).expect("the fixture is removed");
    }

    #[test]
    fn output_paths_cannot_escape_lexically() {
        let root = fixture_root("escape");
        fs::create_dir_all(&root).expect("the fixture root is created");
        let paths = BTreeSet::from([root.join("../outside/generated.rs")]);

        let error = validate_output_paths(&root, &paths)
            .expect_err("a parent traversal cannot become a destination");
        assert_eq!(error.error.kind(), std::io::ErrorKind::InvalidInput);

        fs::remove_dir_all(&root).expect("the fixture is removed");
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_generated_directory_is_never_read_or_removed() {
        use std::os::unix::fs::symlink;

        let root = fixture_root("linked-root");
        let external = fixture_root("linked-external");
        fs::create_dir_all(root.join("sources/example/src"))
            .expect("the workspace fixture is created");
        fs::create_dir_all(&external).expect("the external fixture is created");
        let external_file = external.join("keep.rs");
        fs::write(
            &external_file,
            "//! Generated by `make codegen`. Do not edit.\n",
        )
        .expect("the external file is written");
        symlink(&external, root.join("sources/example/src/generated"))
            .expect("the generated directory is linked");

        let error = reconcile_projection_artifacts(&root, &BTreeSet::new(), Mode::Generate)
            .expect_err("the reconciler refuses linked generated directories");
        assert_eq!(error.error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(
            external_file.exists(),
            "nothing outside the workspace changed"
        );

        fs::remove_dir_all(&root).expect("the workspace fixture is removed");
        fs::remove_dir_all(&external).expect("the external fixture is removed");
    }

    #[cfg(unix)]
    #[test]
    fn an_expected_output_never_writes_through_a_linked_parent() {
        use std::os::unix::fs::symlink;

        let root = fixture_root("linked-output-root");
        let external = fixture_root("linked-output-external");
        fs::create_dir_all(&root).expect("the workspace fixture is created");
        fs::create_dir_all(&external).expect("the external fixture is created");
        symlink(&external, root.join("generated")).expect("the output parent is linked");
        let external_file = external.join("model.rs");

        let error = reconcile_expected_artifacts(
            &root,
            vec![(root.join("generated/model.rs"), "outside".to_owned())],
            Mode::Generate,
        )
        .expect_err("the writer refuses a linked output parent");
        assert_eq!(error.failure.error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(
            !external_file.exists(),
            "nothing outside the workspace changed"
        );

        fs::remove_dir_all(&root).expect("the workspace fixture is removed");
        fs::remove_dir_all(&external).expect("the external fixture is removed");
    }

    #[test]
    fn generator_ownership_requires_the_complete_header() {
        assert!(has_projection_generator_header(
            "//! Generated from `a.textpb` by `make codegen`. Do not edit.\n"
        ));
        assert!(!has_projection_generator_header(
            "//! Generated by another tool. Do not edit.\n"
        ));
        assert!(!has_projection_generator_header(
            "// hand-written mention of `make codegen`; Do not edit\n"
        ));
    }
}
