//! Gate's files stay short: no `.rs` file in Gate's code is over 500
//! lines, docs and tests included.
//!
//! Gate's code is its crates under `crates/gate`, its settings and
//! record types, and the app's modules that run it (see `GATE_DIRS`).

use std::fs;
use std::path::{Path, PathBuf};

/// Most lines a file in Gate's code may have.
const MAX_LINES: usize = 500;

/// Folders, relative to the workspace root, whose every `.rs` file is
/// Gate's.
const GATE_DIRS: &[&str] = &[
    "crates/gate",
    "crates/config/aura-config/src/gate",
    "crates/core/aura-core-types/src/types/gate",
    "crates/surface/aura-app/src/event_loop/gate",
    "crates/surface/aura-app/src/headless/gate_replay",
];

/// Every `.rs` file under `dir`, at any depth.
fn rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("a readable folder entry").path();
        if path.is_dir() {
            rust_files(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

#[test]
fn no_gate_file_is_over_500_lines() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for dir in GATE_DIRS {
        rust_files(&root.join(dir), &mut files);
    }
    assert!(files.len() > GATE_DIRS.len(), "found only {files:?}");
    let long: Vec<String> = files
        .iter()
        .filter_map(|path| {
            let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
            let lines = text.lines().count();
            let shown = path.strip_prefix(root).unwrap_or(path).display();
            (lines > MAX_LINES).then(|| format!("{shown}: {lines} lines"))
        })
        .collect();
    assert!(
        long.is_empty(),
        "over {MAX_LINES} lines:\n{}",
        long.join("\n")
    );
}
