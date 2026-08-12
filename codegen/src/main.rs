// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Command-line driver for the schema compiler, invoked by `make codegen` and
//! `make check-codegen`.

// A command-line tool reports on stdout and stderr; the workspace lint that
// keeps printing out of library code does not apply to this target.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::process::ExitCode;

use nv_telemetry_codegen::Mode;
use nv_telemetry_codegen::Outcome;

const USAGE: &str = "\
usage: nv-telemetry-codegen <command>

  generate    rewrite the generated trees
  --check     report whether the generated trees are up to date

Generated output includes the contract lock, validated model and wire types,
projection modules, and provenance. All are checked in and compared byte for
byte.
";

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);

    let mode = match args.next().as_deref().and_then(std::ffi::OsStr::to_str) {
        Some("generate") => Mode::Generate,
        Some("--check" | "check") => Mode::Check,
        Some(other) => {
            eprintln!("nv-telemetry-codegen: unknown argument `{other}`\n\n{USAGE}");
            return ExitCode::from(2);
        }
        None => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    if args.next().is_some() {
        eprintln!("nv-telemetry-codegen: expected exactly one argument\n\n{USAGE}");
        return ExitCode::from(2);
    }

    match nv_telemetry_codegen::run(mode) {
        Ok(outcome) => report(mode, &outcome),
        Err(error) => {
            eprintln!("nv-telemetry-codegen: {error}");
            ExitCode::FAILURE
        }
    }
}

fn report(mode: Mode, outcome: &Outcome) -> ExitCode {
    match mode {
        Mode::Generate => {
            println!(
                "nv-telemetry-codegen: {} message(s) examined, {} file(s) written, {} orphan(s) removed",
                outcome.examined,
                outcome.written.len(),
                outcome.removed.len()
            );
            for path in &outcome.written {
                println!("  wrote {}", path.display());
            }
            for path in &outcome.removed {
                println!("  removed generated orphan {}", path.display());
            }
            if outcome
                .written
                .iter()
                .any(|path| path.ends_with("contract.lock"))
            {
                println!(
                    "review the contract lock diff: it records semantics no other gate checks"
                );
            }
            if outcome.unrecognized.is_empty() {
                ExitCode::SUCCESS
            } else {
                eprintln!(
                    "nv-telemetry-codegen: preserved {} unrecognized file(s) under a generated directory",
                    outcome.unrecognized.len()
                );
                for path in &outcome.unrecognized {
                    eprintln!("  {}", path.display());
                }
                ExitCode::FAILURE
            }
        }
        Mode::Check if outcome.stale.is_empty() && outcome.unrecognized.is_empty() => {
            println!(
                "nv-telemetry-codegen: generated tree is up to date ({} message(s) examined)",
                outcome.examined
            );
            ExitCode::SUCCESS
        }
        Mode::Check => {
            eprintln!(
                "nv-telemetry-codegen: {} generated file(s) differ from what the compiler \
                 would write",
                outcome.stale.len()
            );
            for path in &outcome.stale {
                eprintln!("  {}", path.display());
            }
            if !outcome.unrecognized.is_empty() {
                eprintln!(
                    "nv-telemetry-codegen: {} unrecognized file(s) were preserved under a generated directory",
                    outcome.unrecognized.len()
                );
                for path in &outcome.unrecognized {
                    eprintln!("  {}", path.display());
                }
            }
            eprintln!(
                "\n`make codegen` regenerates them. Usually the schema changed and the \
                 tree\ndid not; a stray `cargo fmt` on stable can also rewrite generated \
                 files,\nwhich regenerating puts back.\n\nRead the resulting diff rather \
                 than just committing it. The contract lock\nrecords semantics no other \
                 gate checks — numbers, types, presence,\nannotations, enum values, and \
                 oneofs — so for annotation changes it is the\nonly review surface there \
                 is."
            );
            ExitCode::FAILURE
        }
    }
}
