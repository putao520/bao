// @trace REQ-CLI-002 [test:#17-G-EXAMPLES] [level:integration]
// Examples compile-drift lock (#17-G): the examples/ directory is
// documentation surface — an umbrella API rename that breaks an example must
// fail a TEST, not the next user who runs it. Every subdirectory carrying a
// Cargo.toml must `cargo check` against the CURRENT tree (path-dep
// `bao-core` = ../../src/bao), and the discovered-example count is pinned
// from below (a silently deleted example also fails here).
//
// Carrier choice (lightest): one test, sequential `cargo check` per example
// (no codegen, no link — API-drift is a type-check property). The nested
// cargo inherits CARGO_TARGET_DIR from the outer cargo environment, so the
// engine build is warm; a concurrent build by another agent merely makes
// this test wait on the shared package lock.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Minimum expected example count — bump when adding an example (the floor
/// catches silent deletions; discovery is by Cargo.toml presence).
const MIN_EXAMPLES: usize = 8;

fn examples_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root from src/bao")
        .join("examples")
}

#[test]
fn examples_compile_against_current_umbrella_api() {
    let root = examples_root();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("read {}: {e}", root.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("Cargo.toml").is_file())
        .collect();
    dirs.sort();
    assert!(
        dirs.len() >= MIN_EXAMPLES,
        "expected at least {MIN_EXAMPLES} examples, found {} — silently dropped?",
        dirs.len()
    );

    for dir in &dirs {
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("<unnamed>");
        let manifest = dir.join("Cargo.toml");
        let out = Command::new("cargo")
            .arg("check")
            .arg("--manifest-path")
            .arg(&manifest)
            .output()
            .unwrap_or_else(|e| panic!("spawn cargo for {name}: {e}"));
        assert!(
            out.status.success(),
            "example {name} no longer compiles against the current API \
             (documentation drift). Output:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
