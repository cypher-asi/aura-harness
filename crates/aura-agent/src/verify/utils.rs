//! File snapshot/rollback utilities, build command inference, and context
//! snapshot helpers for build-fix prompts.

use std::collections::HashSet;
use std::path::Path;

use tracing::{info, warn};

use crate::file_ops::{self, FileOp};

use super::error_types::parse_error_references;
use super::signatures::parse_individual_error_signatures;

pub const BUILD_FIX_SNAPSHOT_BUDGET: usize = 30_000;

/// Pre-fix file content captured for rollback on stagnation.
pub struct FileSnapshot {
    pub path: String,
    pub content: Option<String>,
}

/// Snapshot the current on-disk content of every file touched by `file_ops`.
pub fn snapshot_modified_files(project_root: &Path, file_ops: &[FileOp]) -> Vec<FileSnapshot> {
    let mut seen = HashSet::new();
    let mut snapshots = Vec::new();
    for op in file_ops {
        let path = match op {
            FileOp::Create { path, .. }
            | FileOp::Modify { path, .. }
            | FileOp::SearchReplace { path, .. }
            | FileOp::Delete { path } => path,
        };
        if !seen.insert(path.clone()) {
            continue;
        }
        let full_path = project_root.join(path);
        let content = std::fs::read_to_string(&full_path).ok();
        snapshots.push(FileSnapshot {
            path: path.clone(),
            content,
        });
    }
    snapshots
}

/// Restore files to a previously captured snapshot.
pub async fn rollback_to_snapshot(project_root: &Path, snapshots: &[FileSnapshot]) {
    for snap in snapshots {
        let full_path = project_root.join(&snap.path);
        match &snap.content {
            Some(content) => {
                if let Err(e) = tokio::fs::write(&full_path, content).await {
                    warn!(path = %snap.path, error = %e, "failed to rollback file");
                }
            }
            None => {
                if let Err(e) = tokio::fs::remove_file(&full_path).await {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        warn!(path = %snap.path, error = %e, "failed to delete file during rollback");
                    }
                }
            }
        }
    }
}

/// Rewrite known server-starting commands to their build/check equivalents.
pub fn auto_correct_build_command(cmd: &str) -> Option<String> {
    let trimmed = cmd.trim();
    if trimmed == "cargo run" || trimmed.starts_with("cargo run ") {
        let mut corrected = trimmed.replacen("cargo run", "cargo build", 1);
        if let Some(idx) = corrected.find(" -- ") {
            corrected.truncate(idx);
        } else if corrected.ends_with(" --") {
            corrected.truncate(corrected.len() - 3);
        }
        return Some(corrected);
    }
    if trimmed == "npm start" {
        return Some("npm run build".to_string());
    }
    if trimmed.contains("runserver") {
        return Some(trimmed.replace("runserver", "check"));
    }
    None
}

/// Infer a default build-check command from project manifest files.
pub fn infer_default_build_command(project_root: &Path) -> Option<String> {
    if project_root.join("Cargo.toml").is_file() {
        return Some("cargo check --workspace --tests".to_string());
    }
    if project_root.join("package.json").is_file() {
        return Some("npm run build --if-present".to_string());
    }
    if project_root.join("pyproject.toml").is_file()
        || project_root.join("requirements.txt").is_file()
    {
        return Some("python -m compileall .".to_string());
    }
    None
}

/// Infer a default full-suite test command from project manifest files.
///
/// Returns `None` when no recognised manifest is present; the caller should
/// then decide whether to skip the test gate (analysis-only project) or
/// abort with a configuration error.
pub fn infer_default_test_command(project_root: &Path) -> Option<String> {
    if project_root.join("Cargo.toml").is_file() {
        return Some("cargo test --workspace --all-features".to_string());
    }
    if project_root.join("package.json").is_file() {
        return Some("npm test --silent --if-present".to_string());
    }
    if project_root.join("pyproject.toml").is_file()
        || project_root.join("requirements.txt").is_file()
    {
        return Some("python -m pytest -q".to_string());
    }
    None
}

/// Build a codebase snapshot for a build-fix prompt by reading error source
/// files fresh from disk and supplementing with relevant project files.
pub fn build_error_context_snapshot(
    project_root: &Path,
    build_stderr: &str,
    budget: usize,
) -> String {
    let error_refs = parse_error_references(build_stderr);
    let fresh_error_files = file_ops::resolve_error_source_files(project_root, &error_refs, budget);

    if fresh_error_files.is_empty() {
        file_ops::read_relevant_files(&project_root.display().to_string(), budget)
            .unwrap_or_default()
    } else {
        let remaining_budget = budget.saturating_sub(fresh_error_files.len());
        if remaining_budget > 2_000 {
            let supplemental = file_ops::read_relevant_files(
                &project_root.display().to_string(),
                remaining_budget,
            )
            .unwrap_or_default();
            if supplemental.is_empty() {
                fresh_error_files
            } else {
                format!("{fresh_error_files}\n{supplemental}")
            }
        } else {
            fresh_error_files
        }
    }
}

/// Returns true if all current errors are pre-existing (present in baseline).
pub fn all_errors_in_baseline(baseline: &HashSet<String>, stderr: &str) -> bool {
    if baseline.is_empty() {
        return false;
    }
    let current_errors = parse_individual_error_signatures(stderr);
    if current_errors.is_empty() {
        return false;
    }
    let new_errors: HashSet<_> = current_errors.difference(baseline).cloned().collect();
    if new_errors.is_empty() {
        info!(
            pre_existing = current_errors.len(),
            "all build errors are pre-existing (baseline), treating as passed"
        );
        return true;
    }
    false
}
