//! Embeds the example coding tasks the BENCH tab offers: every task of
//! `evals/coding`, `evals/gate_cost`, `evals/gate_hard` and
//! `evals/polyglot`, as
//! `$OUT_DIR/bench_pack.rs` (see `src/bench_pack.rs`).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Suites in the order the BENCH tab offers their tasks: easiest first.
const SUITES: [&str; 4] = ["coding", "gate_cost", "gate_hard", "polyglot"];

fn files(dir: &Path) -> Vec<(String, PathBuf)> {
    if !dir.is_dir() {
        return Vec::new();
    }
    let mut out: Vec<(String, PathBuf)> = walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_entry(|e| e.file_name() != "target")
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            let relative = e
                .path()
                .strip_prefix(dir)
                .expect("walked inside dir")
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            (relative, e.path().to_path_buf())
        })
        .collect();
    out.sort();
    out
}

fn list(out: &mut String, files: &[(String, PathBuf)]) {
    out.push_str("&[");
    for (relative, path) in files {
        let _ = write!(
            out,
            "({relative:?}, include_bytes!({:?})),",
            path.display().to_string()
        );
    }
    out.push(']');
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let evals = manifest.join("../../../evals");
    println!("cargo:rerun-if-changed={}", evals.display());
    let mut out = String::from("&[\n");
    for suite in SUITES {
        let root = evals.join(suite);
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut tasks: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|dir| dir.join("task.toml").is_file())
            .collect();
        tasks.sort();
        for dir in tasks {
            let slug = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let _ = write!(
                out,
                "PackEntry {{ suite: {suite:?}, slug: {slug:?}, toml: include_str!({:?}), workspace: ",
                dir.join("task.toml").display().to_string()
            );
            list(&mut out, &files(&dir.join("workspace")));
            out.push_str(", checks: ");
            list(&mut out, &files(&dir.join("checks")));
            out.push_str(" },\n");
        }
    }
    out.push(']');
    let target =
        PathBuf::from(std::env::var("OUT_DIR").expect("set by cargo")).join("bench_pack.rs");
    std::fs::write(target, out).expect("writing bench_pack.rs");
}
