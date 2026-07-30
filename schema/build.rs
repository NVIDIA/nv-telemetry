// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

// Cargo directives are delivered on stdout, so the workspace lint that keeps
// printing out of library code does not apply here.
#![allow(clippy::print_stdout)]

use std::env;
use std::error::Error;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

const PROTO_ROOT: &str = "proto";
const DESCRIPTOR_SET: &str = "nv_telemetry.bin";

fn main() -> Result<(), Box<dyn Error>> {
    let root = Path::new(PROTO_ROOT);

    let mut files = Vec::new();
    collect_protos(root, &mut files)?;
    // Sorted so the descriptor set does not depend on directory iteration
    // order, which differs between filesystems and would make the generated
    // tree's staleness check fail for no reason.
    files.sort();

    println!("cargo:rerun-if-changed={PROTO_ROOT}");
    for file in &files {
        println!("cargo:rerun-if-changed={}", file.display());
    }

    let relative = files
        .iter()
        .map(|file| file.strip_prefix(root).map(Path::to_path_buf))
        .collect::<Result<Vec<_>, _>>()?;

    let mut compiler = protox::Compiler::new([root])?;
    // Imports are included so that a consumer decoding these bytes gets a
    // self-contained pool, able to resolve the annotation extensions rather
    // than only naming them.
    compiler.include_imports(true);
    compiler.include_source_info(false);
    compiler.open_files(relative)?;

    // Deliberately not `protox::compile(..).encode_to_vec()`. That path goes
    // through prost_types::FileDescriptorSet, whose options messages have
    // nowhere to store extensions, so every custom option is dropped without
    // an error. The annotations would then read back as their defaults, which
    // silently turns every invariant off. This method encodes through
    // reflection and keeps them.
    let encoded = compiler.encode_file_descriptor_set();

    let out = PathBuf::from(env::var("OUT_DIR")?).join(DESCRIPTOR_SET);
    fs::write(out, encoded)?;

    Ok(())
}

fn collect_protos(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();

        // Deliberately not `is_dir`/`is_file`, which follow links. A symlinked
        // directory would recurse until the stack ran out if it pointed at an
        // ancestor, and one pointing outside the checkout would make the
        // descriptor set depend on the machine rather than on the tree.
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_symlink() {
            continue;
        }

        if metadata.is_dir() {
            collect_protos(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "proto") {
            out.push(path);
        }
    }
    Ok(())
}
