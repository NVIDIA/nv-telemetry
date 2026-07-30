// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The nv-telemetry schema compiler.
//!
//! Turns the annotated schema and its projection manifests into Rust. It is a
//! build-time tool: because generated code is checked in, nothing that
//! consumes the data plane depends on this crate.
//!
//! Generated output is deterministic, so a codegen change is reviewable as a
//! schema-shaped diff, and unchanged output is not rewritten. An annotation
//! the compiler cannot honor is an error rather than a warning; it never emits
//! silently degraded code.

use std::fmt;
use std::fs;
use std::io;
use std::path::Path;
use std::path::PathBuf;

use prost_reflect::DescriptorPool;

pub mod hash;
pub mod lint;
pub mod lock;
pub mod options;
pub mod projection;
pub mod provenance;
pub mod validate;
pub mod wrapper;

/// The observation contract. Only this package and its sub-packages are
/// subject to the invariant rules: the annotation and manifest packages
/// describe configuration, where a zero value cannot be mistaken for an
/// observation that was never made.
pub const CONTRACT_PACKAGE: &str = "nv.telemetry.v1";

/// Path of the checked-in contract lock, relative to this crate.
const LOCK_PATH: &str = "../schema/contract.lock";

/// Whether `package` is part of the observation contract.
///
/// Matched as a prefix rather than for equality, so that a future
/// `nv.telemetry.v1.gpu` is covered rather than silently unchecked.
#[must_use]
pub fn is_contract_package(package: &str) -> bool {
    package == CONTRACT_PACKAGE || package.starts_with(&format!("{CONTRACT_PACKAGE}."))
}

/// What the compiler was asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Rewrite the generated trees.
    Generate,
    /// Report whether the generated trees are up to date, changing nothing.
    Check,
}

/// What the compiler did.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct Outcome {
    /// Messages examined in the contract package.
    pub examined: usize,
    /// Files written, in `Generate` mode.
    pub written: Vec<PathBuf>,
    /// Files whose content on disk differs from what would be generated.
    pub stale: Vec<PathBuf>,
}

/// Why compilation could not proceed.
#[derive(Debug)]
pub enum Error {
    /// The embedded descriptor set could not be decoded.
    Pool(prost_reflect::DescriptorError),
    /// An annotation the compiler reads is absent, misshapen, or was lost.
    Vocabulary(options::VocabularyError),
    /// The schema violates a rule the compiler enforces.
    Schema(Vec<lint::Violation>),
    /// A generated file could not be read or written.
    Io(PathBuf, io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pool(error) => write!(f, "descriptor set could not be decoded: {error}"),
            Self::Vocabulary(error) => write!(f, "{error}"),
            Self::Schema(violations) => {
                writeln!(f, "schema rejected by {} rule(s):", violations.len())?;
                for violation in violations {
                    writeln!(f, "  {violation}")?;
                }
                Ok(())
            }
            Self::Io(path, error) => write!(f, "{}: {error}", path.display()),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Pool(error) => Some(error),
            Self::Vocabulary(error) => Some(error),
            Self::Io(_, error) => Some(error),
            Self::Schema(_) => None,
        }
    }
}

/// Decodes the schema shipped by `nv-telemetry-schema`.
///
/// # Errors
///
/// Returns [`Error::Pool`] if the embedded descriptor set is not a valid
/// `FileDescriptorSet`.
pub fn pool() -> Result<DescriptorPool, Error> {
    DescriptorPool::decode(nv_telemetry_schema::DESCRIPTOR_SET).map_err(Error::Pool)
}

/// Runs the compiler over the shipped schema.
///
/// # Errors
///
/// Returns [`Error::Pool`] if the schema cannot be decoded,
/// [`Error::Vocabulary`] if the annotations are not as the compiler reads
/// them, [`Error::Schema`] if the schema breaks an invariant rule, or
/// [`Error::Io`] if a generated file cannot be read or written.
///
/// Backward compatibility is not judged here; `buf breaking` owns it.
pub fn run(mode: Mode) -> Result<Outcome, Error> {
    let pool = pool()?;
    let vocabulary = options::Vocabulary::resolve(&pool).map_err(Error::Vocabulary)?;

    let violations = lint::presence(&pool, &vocabulary);
    if !violations.is_empty() {
        return Err(Error::Schema(violations));
    }

    let examined = pool
        .all_messages()
        .filter(|message| is_contract_package(message.package_name()) && !message.is_map_entry())
        .count();

    let snapshot = lock::Snapshot::capture(&pool, &vocabulary);
    let lock_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(LOCK_PATH);

    let committed = match fs::read_to_string(&lock_path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(Error::Io(lock_path, error)),
    };

    let rendered = snapshot.render();
    let up_to_date = committed.as_deref() == Some(rendered.as_str());

    let mut outcome = Outcome {
        examined,
        written: Vec::new(),
        stale: Vec::new(),
    };

    match mode {
        Mode::Check if !up_to_date => outcome.stale.push(lock_path),
        Mode::Check => {}
        Mode::Generate if up_to_date => {}
        Mode::Generate => {
            fs::write(&lock_path, &rendered)
                .map_err(|error| Error::Io(lock_path.clone(), error))?;
            outcome.written.push(lock_path);
        }
    }

    Ok(outcome)
}
