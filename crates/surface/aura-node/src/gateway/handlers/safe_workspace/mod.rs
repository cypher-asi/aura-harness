//! Hosted per-session Git worktrees and recoverable checkpoints.
//!
//! The hosted local harness owns its filesystem namespace, so Aura OS cannot
//! safely create or inspect these worktrees itself. These protected endpoints
//! keep all Git and filesystem operations beside the runtime that executes the
//! agent. The parent runtime request receives the resulting path; child agents
//! continue to inherit that same parent workspace without any protocol change.
//!
//! This module is a declared invariant §1 infrastructure exception: it may
//! run local-only Git lifecycle commands for worktree isolation and
//! checkpoints. [`git::local_git_command`] enforces a closed
//! subcommand allowlist and disables terminal prompting; network-capable Git
//! operations such as clone, fetch, pull, push, and remote are not permitted.
//! All Git process spawning lives in the `git` submodule.

mod checkpoints;
mod git;
mod handlers;
mod setup;
mod storage;

pub(in crate::gateway) use handlers::{
    apply_safe_workspace_handler, prepare_safe_workspace_handler, restore_safe_workspace_handler,
    safe_workspace_diff_handler, safe_workspace_status_handler,
};
pub(in crate::gateway) use setup::delete_project_safe_workspaces;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;

const SAFE_WORKSPACES_DIR: &str = "safe-workspaces";
const WORKTREE_DIR: &str = "repo";
const METADATA_FILE: &str = "workspace.json";
const CHECKPOINT_STORE_DIR: &str = "checkpoints.git";
const CHECKPOINT_INDEX_FILE: &str = "checkpoint.index";
const CHECKPOINT_REF: &str = "refs/aura/session";
const MAX_CHECKPOINTS_RETURNED: usize = 20;
const MAX_SNAPSHOT_FILES: usize = 50_000;
const MAX_DIFF_BYTES: usize = 200_000;
const MAX_UNTRACKED_COPY_BYTES: u64 = 10 * 1024 * 1024;
const LOCK_ATTEMPTS: usize = 200;
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(25);
const STALE_LOCK_AGE: Duration = Duration::from_secs(120);

#[derive(Debug, Error)]
enum SafeWorkspaceError {
    #[error("safe workspace requires a local Git repository: {0}")]
    Unsupported(String),
    #[error("safe workspace is busy; try again in a moment")]
    Busy,
    #[error("safe workspace changes conflict with the current project: {0}")]
    Conflict(String),
    #[error("safe workspace metadata is invalid: {0}")]
    InvalidMetadata(String),
    #[error("Git command failed: {0}")]
    Git(String),
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("workspace metadata could not be encoded: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceMetadata {
    version: u8,
    project_id: String,
    session_id: String,
    source_repo: PathBuf,
    source_subpath: PathBuf,
    workspace_root: PathBuf,
    workspace_path: PathBuf,
    base_commit: String,
    created_at: String,
    /// Latest isolated checkpoint successfully applied back to the source
    /// project. The next handoff diffs from here instead of replaying changes
    /// that were already applied.
    #[serde(default)]
    applied_checkpoint: Option<String>,
}

impl WorkspaceMetadata {
    /// The session folder the managed worktree sits in, which also holds
    /// the checkpoint store.
    fn session_dir(&self) -> Result<&Path, SafeWorkspaceError> {
        self.workspace_root.parent().ok_or_else(|| {
            SafeWorkspaceError::InvalidMetadata(format!(
                "workspace root {} has no session folder",
                self.workspace_root.display()
            ))
        })
    }

    /// The project folder in the source repository this workspace mirrors.
    fn source_display_path(&self) -> String {
        self.source_repo
            .join(&self.source_subpath)
            .to_string_lossy()
            .to_string()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SafeWorkspaceCheckpoint {
    id: String,
    short_id: String,
    created_at: String,
    reason: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SafeWorkspaceStatus {
    enabled: bool,
    workspace_path: Option<String>,
    source_path: Option<String>,
    base_commit: Option<String>,
    created_at: Option<String>,
    checkpoints: Vec<SafeWorkspaceCheckpoint>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SafeWorkspaceDiff {
    checkpoint_id: String,
    stat: String,
    diff: String,
    truncated: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SafeWorkspaceRestoreResult {
    restored_to: String,
    undo_checkpoint_id: String,
    workspace_path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SafeWorkspaceApplyResult {
    applied: bool,
    checkpoint_id: String,
    stat: String,
    source_path: String,
}

#[cfg(test)]
mod tests;
