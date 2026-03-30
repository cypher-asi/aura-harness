//! Shared helpers for build and test fix flows: file op descriptions,
//! codebase snapshots, and fix application with attempt recording.

use std::path::Path;

use tracing::warn;

use crate::file_ops::{self, FileOp};

use super::error_types::BuildFixAttemptRecord;
use super::signatures::normalize_error_signature;
use super::FixProvider;

/// Describe each file op as a human-readable "op_name path" string.
pub fn describe_file_ops(ops: &[FileOp]) -> Vec<String> {
    ops.iter()
        .map(|op| {
            let (op_name, path) = match op {
                FileOp::Create { path, .. } => ("create", path.as_str()),
                FileOp::Modify { path, .. } => ("modify", path.as_str()),
                FileOp::Delete { path } => ("delete", path.as_str()),
                FileOp::SearchReplace { path, .. } => ("search_replace", path.as_str()),
            };
            format!("{op_name} {path}")
        })
        .collect()
}

fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..max]
    }
}

/// Produce a concise diff-style summary of file operations.
pub fn summarize_file_ops(ops: &[FileOp]) -> String {
    ops.iter()
        .map(|op| match op {
            FileOp::SearchReplace {
                path, replacements, ..
            } => {
                let changes: Vec<String> = replacements
                    .iter()
                    .map(|r| {
                        format!(
                            "  - replaced: {:?} -> {:?}",
                            truncate_str(&r.search, 80),
                            truncate_str(&r.replace, 80),
                        )
                    })
                    .collect();
                format!("{}:\n{}", path, changes.join("\n"))
            }
            FileOp::Modify { path, .. } => format!("{}: full rewrite", path),
            FileOp::Create { path, .. } => format!("{}: created", path),
            FileOp::Delete { path } => format!("{}: deleted", path),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Build a codebase snapshot for fix prompts using the `read_relevant_files`
/// budget-based approach.
pub fn build_codebase_snapshot(project_folder: &str, budget: usize) -> String {
    file_ops::read_relevant_files(project_folder, budget).unwrap_or_default()
}

/// Parse an LLM fix response, apply file operations to disk, and record the
/// attempt in the history for stagnation detection.
///
/// Returns `true` when the fix was successfully applied.
#[allow(clippy::too_many_arguments)]
pub async fn apply_fix_and_record(
    base_path: &Path,
    response: &str,
    attempt: u32,
    stderr: &str,
    prior_attempts: &mut Vec<BuildFixAttemptRecord>,
    all_fix_ops: &mut Vec<FileOp>,
    fix_kind: &str,
    fix_provider: &dyn FixProvider,
) -> anyhow::Result<bool> {
    match fix_provider.parse_fix_response(response) {
        Ok(fix_ops) => {
            if let Err(e) = file_ops::apply_file_ops(base_path, &fix_ops).await {
                warn!(
                    attempt, error = %e,
                    "file ops failed during {fix_kind} (likely search-replace mismatch), \
                     treating as failed fix attempt"
                );
                let sig = normalize_error_signature(stderr);
                prior_attempts.push(BuildFixAttemptRecord {
                    stderr: stderr.to_string(),
                    error_signature: sig,
                    files_changed: vec!["(fix did not apply)".into()],
                    changes_summary: String::new(),
                });
                return Ok(false);
            }
            let files_changed = describe_file_ops(&fix_ops);
            let changes_summary = summarize_file_ops(&fix_ops);
            let sig = normalize_error_signature(stderr);
            prior_attempts.push(BuildFixAttemptRecord {
                stderr: stderr.to_string(),
                error_signature: sig,
                files_changed,
                changes_summary,
            });
            all_fix_ops.extend(fix_ops);
            Ok(true)
        }
        Err(e) => {
            warn!(
                attempt, error = %e,
                "failed to parse {fix_kind} response, fix not applied"
            );
            Ok(false)
        }
    }
}
