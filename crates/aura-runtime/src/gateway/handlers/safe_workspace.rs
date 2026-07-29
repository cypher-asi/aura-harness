//! Hosted per-session Git worktrees and recoverable checkpoints.
//!
//! The hosted local harness owns its filesystem namespace, so Aura OS cannot
//! safely create or inspect these worktrees itself. These protected endpoints
//! keep all Git and filesystem operations beside the runtime that executes the
//! agent. The parent runtime request receives the resulting path; child agents
//! continue to inherit that same parent workspace without any protocol change.

use super::super::*;
use axum::extract::Path as AxumPath;
use axum::response::Response;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, SystemTime};
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

struct WorkspaceLock {
    path: PathBuf,
}

impl WorkspaceLock {
    fn acquire(root: &Path) -> Result<Self, SafeWorkspaceError> {
        fs::create_dir_all(root)?;
        let path = root.join(".lock");
        for _ in 0..LOCK_ATTEMPTS {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "{}", std::process::id());
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(&path)
                        .and_then(|metadata| metadata.modified())
                        .ok()
                        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
                        .is_some_and(|age| age > STALE_LOCK_AGE);
                    if stale {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    thread::sleep(LOCK_RETRY_DELAY);
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(SafeWorkspaceError::Busy)
    }
}

impl Drop for WorkspaceLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn session_root(data_dir: &Path, project_id: &str, session_id: &str) -> PathBuf {
    data_dir
        .join(SAFE_WORKSPACES_DIR)
        .join(project_id)
        .join(session_id)
}

fn source_lock_root(data_dir: &Path, project_id: &str) -> PathBuf {
    data_dir
        .join(SAFE_WORKSPACES_DIR)
        .join(project_id)
        .join(".source-lock")
}

/// Find an existing workspace beneath Aura's canonical data directory.
///
/// Axum route IDs have already been parsed as UUID-backed types, but this
/// containment check is intentionally independent of that invariant. It also
/// prevents a locally replaced symlink from redirecting a request outside the
/// managed workspace tree.
fn find_existing_session_root(
    data_dir: &Path,
    project_id: &str,
    session_id: &str,
) -> Result<Option<PathBuf>, SafeWorkspaceError> {
    let canonical_data_dir = data_dir.canonicalize()?;
    let safe_workspaces = match canonical_data_dir.join(SAFE_WORKSPACES_DIR).canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !safe_workspaces.starts_with(&canonical_data_dir) {
        return Err(SafeWorkspaceError::InvalidMetadata(
            "managed workspace directory escaped the harness data directory".to_string(),
        ));
    }

    let requested_root = safe_workspaces.join(project_id).join(session_id);
    let root = match requested_root.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !root.starts_with(&safe_workspaces) {
        return Err(SafeWorkspaceError::InvalidMetadata(
            "managed session directory escaped the safe workspace tree".to_string(),
        ));
    }
    if root
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        != Some(project_id)
        || root.file_name().and_then(|value| value.to_str()) != Some(session_id)
    {
        return Ok(None);
    }

    match fs::symlink_metadata(root.join(METADATA_FILE)) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(Some(root)),
        Ok(_) => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn command_error(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        stderr
    }
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<Output, SafeWorkspaceError> {
    let output = Command::new("git").current_dir(cwd).args(args).output()?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(SafeWorkspaceError::Git(command_error(&output)))
    }
}

fn run_git_with_input(
    cwd: &Path,
    args: &[&str],
    input: &[u8],
) -> Result<Output, SafeWorkspaceError> {
    let mut child = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| SafeWorkspaceError::Git("could not open git stdin".to_string()))?
        .write_all(input)?;
    let output = child.wait_with_output()?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(SafeWorkspaceError::Git(command_error(&output)))
    }
}

fn shadow_command(metadata: &WorkspaceMetadata) -> Command {
    let root = metadata
        .workspace_root
        .parent()
        .expect("managed worktree always has a session root");
    let mut command = Command::new("git");
    command
        .current_dir(&metadata.workspace_root)
        .env("GIT_DIR", root.join(CHECKPOINT_STORE_DIR))
        .env("GIT_WORK_TREE", &metadata.workspace_root)
        .env("GIT_INDEX_FILE", root.join(CHECKPOINT_INDEX_FILE))
        .env("GIT_AUTHOR_NAME", "Aura Safe Workspace")
        .env("GIT_AUTHOR_EMAIL", "safe-workspace@aura.local")
        .env("GIT_COMMITTER_NAME", "Aura Safe Workspace")
        .env("GIT_COMMITTER_EMAIL", "safe-workspace@aura.local");
    command
}

fn run_shadow(
    metadata: &WorkspaceMetadata,
    args: &[&str],
    allowed_failure: bool,
) -> Result<Output, SafeWorkspaceError> {
    let output = shadow_command(metadata).args(args).output()?;
    if output.status.success() || allowed_failure {
        Ok(output)
    } else {
        Err(SafeWorkspaceError::Git(command_error(&output)))
    }
}

fn stdout_trimmed(output: Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn validate_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

fn copy_untracked_files(
    source_repo: &Path,
    workspace_root: &Path,
) -> Result<(), SafeWorkspaceError> {
    let output = run_git(
        source_repo,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    for raw_path in output.stdout.split(|byte| *byte == 0) {
        if raw_path.is_empty() {
            continue;
        }
        let relative = PathBuf::from(String::from_utf8_lossy(raw_path).as_ref());
        if !validate_relative_path(&relative) {
            continue;
        }
        let source = source_repo.join(&relative);
        let metadata = match fs::symlink_metadata(&source) {
            Ok(metadata) if metadata.file_type().is_file() => metadata,
            _ => continue,
        };
        if metadata.len() > MAX_UNTRACKED_COPY_BYTES {
            continue;
        }
        let destination = workspace_root.join(&relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&source, &destination)?;
        fs::set_permissions(&destination, metadata.permissions())?;
    }
    Ok(())
}

fn initialize_shadow_store(metadata: &WorkspaceMetadata) -> Result<(), SafeWorkspaceError> {
    let root = metadata
        .workspace_root
        .parent()
        .ok_or_else(|| SafeWorkspaceError::InvalidMetadata("missing session root".to_string()))?;
    let store = root.join(CHECKPOINT_STORE_DIR);
    if !store.join("HEAD").exists() {
        fs::create_dir_all(&store)?;
        let output = Command::new("git")
            .args(["init", "--bare", store.to_string_lossy().as_ref()])
            .output()?;
        if !output.status.success() {
            return Err(SafeWorkspaceError::Git(command_error(&output)));
        }
        let info = store.join("info");
        fs::create_dir_all(&info)?;
        fs::write(
            info.join("exclude"),
            ".git\nnode_modules/\ntarget/\ndist/\nbuild/\n.next/\ncoverage/\n*.log\n.env\n.env.*\n",
        )?;
    }
    Ok(())
}

fn checkpoint_tip(metadata: &WorkspaceMetadata) -> Result<Option<String>, SafeWorkspaceError> {
    let output = run_shadow(
        metadata,
        &[
            "rev-parse",
            "--verify",
            &format!("{CHECKPOINT_REF}^{{commit}}"),
        ],
        true,
    )?;
    Ok(output.status.success().then(|| stdout_trimmed(output)))
}

fn take_checkpoint(
    metadata: &WorkspaceMetadata,
    reason: &str,
) -> Result<String, SafeWorkspaceError> {
    initialize_shadow_store(metadata)?;

    let count_output = run_git(
        &metadata.workspace_root,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    )?;
    let file_count = count_output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .count();
    if file_count > MAX_SNAPSHOT_FILES {
        return Err(SafeWorkspaceError::Unsupported(format!(
            "workspace has {file_count} files; checkpoint limit is {MAX_SNAPSHOT_FILES}"
        )));
    }

    let parent = checkpoint_tip(metadata)?;
    let index_path = metadata
        .workspace_root
        .parent()
        .expect("managed worktree always has a session root")
        .join(CHECKPOINT_INDEX_FILE);
    if let Some(parent) = parent.as_deref() {
        run_shadow(metadata, &["read-tree", parent], false)?;
    } else if index_path.exists() {
        fs::remove_file(&index_path)?;
    }

    run_shadow(metadata, &["add", "-A", "--", "."], false)?;
    let tree = stdout_trimmed(run_shadow(metadata, &["write-tree"], false)?);

    if let Some(parent) = parent.as_deref() {
        let parent_tree = stdout_trimmed(run_shadow(
            metadata,
            &["rev-parse", &format!("{parent}^{{tree}}")],
            false,
        )?);
        if parent_tree == tree {
            return Ok(parent.to_string());
        }
    }

    let mut args = vec!["commit-tree", tree.as_str(), "-m", reason, "--no-gpg-sign"];
    if let Some(parent) = parent.as_deref() {
        args.splice(2..2, ["-p", parent]);
    }
    let commit = stdout_trimmed(run_shadow(metadata, &args, false)?);
    let mut update_args = vec!["update-ref", CHECKPOINT_REF, commit.as_str()];
    if let Some(parent) = parent.as_deref() {
        update_args.push(parent);
    }
    run_shadow(metadata, &update_args, false)?;
    Ok(commit)
}

fn read_metadata(root: &Path) -> Result<WorkspaceMetadata, SafeWorkspaceError> {
    let mut bytes = Vec::new();
    File::open(root.join(METADATA_FILE))?.read_to_end(&mut bytes)?;
    let metadata: WorkspaceMetadata = serde_json::from_slice(&bytes)?;
    let expected_root = root.join(WORKTREE_DIR);
    let stored_root = metadata.workspace_root.canonicalize()?;
    let expected_root = expected_root.canonicalize()?;
    if metadata.version != 1 || stored_root != expected_root {
        return Err(SafeWorkspaceError::InvalidMetadata(
            "managed path does not match its session directory".to_string(),
        ));
    }
    Ok(metadata)
}

fn write_metadata(root: &Path, metadata: &WorkspaceMetadata) -> Result<(), SafeWorkspaceError> {
    let encoded = serde_json::to_vec_pretty(metadata)?;
    let temporary = root.join(format!("{METADATA_FILE}.tmp"));
    fs::write(&temporary, encoded)?;
    fs::rename(temporary, root.join(METADATA_FILE))?;
    Ok(())
}

fn remove_managed_entry(path: &Path) -> Result<(), SafeWorkspaceError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// A failed first-time setup can leave a registered worktree or partial
/// shadow repository without metadata. No chat turn can have received that
/// path yet, so it is safe to discard only these managed entries and retry.
fn clean_incomplete_workspace(
    source_repo: &Path,
    root: &Path,
    workspace_root: &Path,
) -> Result<(), SafeWorkspaceError> {
    if workspace_root.exists() {
        let workspace_arg = workspace_root.to_string_lossy().to_string();
        let _ = Command::new("git")
            .current_dir(source_repo)
            .args(["worktree", "remove", "--force", &workspace_arg])
            .output()?;
        remove_managed_entry(workspace_root)?;
    }
    run_git(source_repo, &["worktree", "prune"])?;
    remove_managed_entry(&root.join(CHECKPOINT_STORE_DIR))?;
    remove_managed_entry(&root.join(CHECKPOINT_INDEX_FILE))?;
    remove_managed_entry(&root.join(format!("{METADATA_FILE}.tmp")))?;
    Ok(())
}

fn prepare_workspace_blocking(
    data_dir: &Path,
    project_id: &str,
    session_id: &str,
    source_path: &Path,
) -> Result<WorkspaceMetadata, SafeWorkspaceError> {
    let root = session_root(data_dir, project_id, session_id);
    let _lock = WorkspaceLock::acquire(&root)?;
    if root.join(METADATA_FILE).exists() {
        let metadata = read_metadata(&root)?;
        if metadata.project_id != project_id || metadata.session_id != session_id {
            return Err(SafeWorkspaceError::InvalidMetadata(
                "project or session id mismatch".to_string(),
            ));
        }
        if !metadata.workspace_path.is_dir() {
            return Err(SafeWorkspaceError::InvalidMetadata(
                "managed worktree no longer exists".to_string(),
            ));
        }
        return Ok(metadata);
    }

    // Two sessions may opt in at the same moment. Serialise the one-time Git
    // bootstrap for browser-imported projects and source worktree mutation
    // across all sessions belonging to this project.
    let _source_lock = WorkspaceLock::acquire(&source_lock_root(data_dir, project_id))?;
    ensure_source_git_repository(source_path)?;
    let source_path = source_path.canonicalize().map_err(|error| {
        SafeWorkspaceError::Unsupported(format!("{} ({error})", source_path.display()))
    })?;
    let source_repo = PathBuf::from(stdout_trimmed(run_git(
        &source_path,
        &["rev-parse", "--show-toplevel"],
    )?))
    .canonicalize()?;
    let source_subpath = source_path
        .strip_prefix(&source_repo)
        .map_err(|_| {
            SafeWorkspaceError::Unsupported("workspace is outside its Git root".to_string())
        })?
        .to_path_buf();
    if root.starts_with(&source_repo) {
        return Err(SafeWorkspaceError::Unsupported(
            "Aura's data directory cannot be inside the project repository".to_string(),
        ));
    }

    let base_commit = stdout_trimmed(run_git(&source_repo, &["rev-parse", "HEAD"])?);
    let workspace_root = root.join(WORKTREE_DIR);
    clean_incomplete_workspace(&source_repo, &root, &workspace_root)?;
    let workspace_arg = workspace_root.to_string_lossy().to_string();
    run_git(
        &source_repo,
        &["worktree", "add", "--detach", &workspace_arg, &base_commit],
    )?;

    let patch = run_git(
        &source_repo,
        &["diff", "--binary", "--full-index", "HEAD", "--", "."],
    )?
    .stdout;
    if !patch.is_empty() {
        run_git_with_input(
            &workspace_root,
            &["apply", "--whitespace=nowarn", "-"],
            &patch,
        )?;
    }
    copy_untracked_files(&source_repo, &workspace_root)?;

    let workspace_path = workspace_root.join(&source_subpath);
    let metadata = WorkspaceMetadata {
        version: 1,
        project_id: project_id.to_string(),
        session_id: session_id.to_string(),
        source_repo,
        source_subpath,
        workspace_root,
        workspace_path,
        base_commit,
        created_at: Utc::now().to_rfc3339(),
        applied_checkpoint: None,
    };
    take_checkpoint(&metadata, "workspace baseline")?;
    write_metadata(&root, &metadata)?;
    Ok(metadata)
}

fn list_checkpoints_blocking(
    metadata: &WorkspaceMetadata,
) -> Result<Vec<SafeWorkspaceCheckpoint>, SafeWorkspaceError> {
    initialize_shadow_store(metadata)?;
    let limit = MAX_CHECKPOINTS_RETURNED.to_string();
    let output = run_shadow(
        metadata,
        &[
            "log",
            CHECKPOINT_REF,
            "--format=%H%x1f%h%x1f%aI%x1f%s",
            "-n",
            &limit,
        ],
        true,
    )?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(4, '\u{1f}');
            Some(SafeWorkspaceCheckpoint {
                id: parts.next()?.to_string(),
                short_id: parts.next()?.to_string(),
                created_at: parts.next()?.to_string(),
                reason: parts.next()?.to_string(),
            })
        })
        .collect())
}

fn validate_checkpoint_id(
    metadata: &WorkspaceMetadata,
    checkpoint_id: &str,
) -> Result<String, SafeWorkspaceError> {
    if !(4..=64).contains(&checkpoint_id.len())
        || !checkpoint_id.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(SafeWorkspaceError::Unsupported(
            "checkpoint id must be a hexadecimal Git object id".to_string(),
        ));
    }
    let resolved = run_shadow(
        metadata,
        &[
            "rev-parse",
            "--verify",
            &format!("{checkpoint_id}^{{commit}}"),
        ],
        false,
    )?;
    let resolved = stdout_trimmed(resolved);
    let ancestor = run_shadow(
        metadata,
        &["merge-base", "--is-ancestor", &resolved, CHECKPOINT_REF],
        true,
    )?;
    if !ancestor.status.success() {
        return Err(SafeWorkspaceError::Unsupported(
            "checkpoint does not belong to this session".to_string(),
        ));
    }
    Ok(resolved)
}

fn checkpoint_diff_blocking(
    metadata: &WorkspaceMetadata,
    checkpoint_id: &str,
) -> Result<SafeWorkspaceDiff, SafeWorkspaceError> {
    let checkpoint_id = validate_checkpoint_id(metadata, checkpoint_id)?;
    run_shadow(metadata, &["add", "-A", "--", "."], false)?;
    let stat = stdout_trimmed(run_shadow(
        metadata,
        &["diff", "--cached", "--stat", &checkpoint_id],
        false,
    )?);
    let output = run_shadow(
        metadata,
        &["diff", "--cached", "--no-color", "--binary", &checkpoint_id],
        false,
    )?;
    if let Some(tip) = checkpoint_tip(metadata)? {
        run_shadow(metadata, &["read-tree", &tip], false)?;
    }
    let truncated = output.stdout.len() > MAX_DIFF_BYTES;
    let diff = String::from_utf8_lossy(&output.stdout[..output.stdout.len().min(MAX_DIFF_BYTES)])
        .to_string();
    Ok(SafeWorkspaceDiff {
        checkpoint_id,
        stat,
        diff,
        truncated,
    })
}

fn restore_checkpoint_blocking(
    root: &Path,
    checkpoint_id: &str,
) -> Result<SafeWorkspaceRestoreResult, SafeWorkspaceError> {
    let _lock = WorkspaceLock::acquire(root)?;
    let metadata = read_metadata(root)?;
    let checkpoint_id = validate_checkpoint_id(&metadata, checkpoint_id)?;
    let undo_checkpoint_id = take_checkpoint(
        &metadata,
        &format!(
            "pre-rollback snapshot (restoring to {})",
            &checkpoint_id[..8]
        ),
    )?;
    run_shadow(
        &metadata,
        &["read-tree", "--reset", "-u", &checkpoint_id],
        false,
    )?;
    Ok(SafeWorkspaceRestoreResult {
        restored_to: checkpoint_id,
        undo_checkpoint_id,
        workspace_path: metadata.workspace_path.to_string_lossy().to_string(),
    })
}

fn first_checkpoint(metadata: &WorkspaceMetadata) -> Result<String, SafeWorkspaceError> {
    let output = run_shadow(
        metadata,
        &["rev-list", "--max-parents=0", CHECKPOINT_REF],
        false,
    )?;
    stdout_trimmed(output)
        .lines()
        .last()
        .map(str::to_string)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| SafeWorkspaceError::InvalidMetadata("baseline checkpoint missing".into()))
}

fn apply_to_source_blocking(root: &Path) -> Result<SafeWorkspaceApplyResult, SafeWorkspaceError> {
    let _lock = WorkspaceLock::acquire(root)?;
    let mut metadata = read_metadata(root)?;
    let project_root = root.parent().ok_or_else(|| {
        SafeWorkspaceError::InvalidMetadata("managed project directory is missing".to_string())
    })?;
    let _source_lock = WorkspaceLock::acquire(&project_root.join(".source-lock"))?;
    let checkpoint_id = take_checkpoint(&metadata, "before applying changes to project")?;
    let baseline = match metadata.applied_checkpoint.as_deref() {
        Some(id) => validate_checkpoint_id(&metadata, id)?,
        None => first_checkpoint(&metadata)?,
    };
    let stat = stdout_trimmed(run_shadow(
        &metadata,
        &["diff", "--stat", &baseline, &checkpoint_id],
        false,
    )?);
    let patch = run_shadow(
        &metadata,
        &[
            "diff",
            "--binary",
            "--full-index",
            &baseline,
            &checkpoint_id,
        ],
        false,
    )?
    .stdout;

    if patch.is_empty() {
        return Ok(SafeWorkspaceApplyResult {
            applied: false,
            checkpoint_id,
            stat,
            source_path: metadata
                .source_repo
                .join(&metadata.source_subpath)
                .to_string_lossy()
                .to_string(),
        });
    }

    if let Err(error) = run_git_with_input(
        &metadata.source_repo,
        &["apply", "--check", "--whitespace=nowarn", "-"],
        &patch,
    ) {
        return Err(SafeWorkspaceError::Conflict(error.to_string()));
    }
    if let Err(error) = run_git_with_input(
        &metadata.source_repo,
        &["apply", "--whitespace=nowarn", "-"],
        &patch,
    ) {
        return Err(SafeWorkspaceError::Conflict(error.to_string()));
    }

    metadata.applied_checkpoint = Some(checkpoint_id.clone());
    write_metadata(root, &metadata)?;
    Ok(SafeWorkspaceApplyResult {
        applied: true,
        checkpoint_id,
        stat,
        source_path: metadata
            .source_repo
            .join(&metadata.source_subpath)
            .to_string_lossy()
            .to_string(),
    })
}

pub(in crate::gateway) async fn delete_project_safe_workspaces(
    data_dir: &Path,
    workspace_key: &str,
) -> Result<(), String> {
    validate_key(workspace_key, "workspace key").map_err(|error| error.to_string())?;
    let project_root = data_dir.join(SAFE_WORKSPACES_DIR).join(workspace_key);
    match tokio::fs::symlink_metadata(&project_root).await {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err("managed Safe Workspace project root must be a real directory".to_string())
        }
        Ok(_) => tokio::fs::remove_dir_all(&project_root)
            .await
            .map_err(|error| format!("failed to delete project Safe Workspaces: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "failed to inspect project Safe Workspaces: {error}"
        )),
    }
}

fn validate_key<'a>(value: &'a str, label: &str) -> Result<&'a str, SafeWorkspaceError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(SafeWorkspaceError::Unsupported(format!(
            "{label} must contain only ASCII letters, numbers, '-' or '_'"
        )));
    }
    Ok(value)
}

fn ensure_source_git_repository(source_path: &Path) -> Result<(), SafeWorkspaceError> {
    let metadata = fs::symlink_metadata(source_path).map_err(|error| {
        SafeWorkspaceError::Unsupported(format!("{} ({error})", source_path.display()))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(SafeWorkspaceError::Unsupported(
            "hosted workspace root must be a real directory".to_string(),
        ));
    }

    let canonical_source = source_path.canonicalize()?;
    let probe = Command::new("git")
        .current_dir(&canonical_source)
        .args(["rev-parse", "--show-toplevel"])
        .output()?;
    let owns_repository = probe.status.success()
        && PathBuf::from(String::from_utf8_lossy(&probe.stdout).trim())
            .canonicalize()
            .ok()
            .as_deref()
            == Some(canonical_source.as_path());

    if !owns_repository {
        run_git(&canonical_source, &["init"])?;
        let git_dir = canonical_source.join(".git");
        fs::create_dir_all(git_dir.join("info"))?;
        fs::write(
            git_dir.join("info").join("exclude"),
            ".env\n.env.*\nnode_modules/\ntarget/\ndist/\nbuild/\n.next/\ncoverage/\n*.log\n",
        )?;
    }

    let head = Command::new("git")
        .current_dir(&canonical_source)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()?;
    if !head.status.success() {
        run_git(&canonical_source, &["add", "-A", "--", "."])?;
        run_git(
            &canonical_source,
            &[
                "-c",
                "user.name=Aura Hosted Workspace",
                "-c",
                "user.email=hosted-workspace@aura.local",
                "commit",
                "--allow-empty",
                "--no-gpg-sign",
                "-m",
                "Aura hosted workspace baseline",
            ],
        )?;
    }
    Ok(())
}

fn workspace_error_response(error: SafeWorkspaceError) -> Response {
    let status = match error {
        SafeWorkspaceError::Unsupported(_) | SafeWorkspaceError::InvalidMetadata(_) => {
            StatusCode::BAD_REQUEST
        }
        SafeWorkspaceError::Busy | SafeWorkspaceError::Conflict(_) => StatusCode::CONFLICT,
        SafeWorkspaceError::Git(_) | SafeWorkspaceError::Io(_) | SafeWorkspaceError::Json(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    (
        status,
        Json(serde_json::json!({
            "ok": false,
            "error": error.to_string(),
        })),
    )
        .into_response()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PrepareSafeWorkspaceResponse {
    workspace_path: String,
}

pub(in crate::gateway) async fn prepare_safe_workspace_handler(
    State(state): State<RouterState>,
    AxumPath((workspace_key, session_id)): AxumPath<(String, String)>,
) -> Response {
    let workspace_key = match validate_key(&workspace_key, "workspace key") {
        Ok(value) => value.to_string(),
        Err(error) => return workspace_error_response(error),
    };
    let session_id = match validate_key(&session_id, "session id") {
        Ok(value) => value.to_string(),
        Err(error) => return workspace_error_response(error),
    };
    let data_dir = state.config.data_dir.clone();
    let source_path = state.config.resolve_workspace_for_project(&workspace_key);
    match tokio::task::spawn_blocking(move || {
        let metadata =
            prepare_workspace_blocking(&data_dir, &workspace_key, &session_id, &source_path)?;
        let root = session_root(&data_dir, &workspace_key, &session_id);
        let _lock = WorkspaceLock::acquire(&root)?;
        take_checkpoint(&metadata, "before chat turn")?;
        Ok::<_, SafeWorkspaceError>(PrepareSafeWorkspaceResponse {
            workspace_path: metadata.workspace_path.to_string_lossy().to_string(),
        })
    })
    .await
    {
        Ok(Ok(result)) => Json(result).into_response(),
        Ok(Err(error)) => workspace_error_response(error),
        Err(error) => workspace_error_response(SafeWorkspaceError::Io(std::io::Error::other(
            format!("safe workspace task failed: {error}"),
        ))),
    }
}

pub(in crate::gateway) async fn safe_workspace_status_handler(
    State(state): State<RouterState>,
    AxumPath((workspace_key, session_id)): AxumPath<(String, String)>,
) -> Response {
    let workspace_key = match validate_key(&workspace_key, "workspace key") {
        Ok(value) => value.to_string(),
        Err(error) => return workspace_error_response(error),
    };
    let session_id = match validate_key(&session_id, "session id") {
        Ok(value) => value.to_string(),
        Err(error) => return workspace_error_response(error),
    };
    let data_dir = state.config.data_dir.clone();
    let root = match find_existing_session_root(&data_dir, &workspace_key, &session_id) {
        Ok(Some(root)) => root,
        Ok(None) => {
            return Json(SafeWorkspaceStatus {
                enabled: false,
                workspace_path: None,
                source_path: None,
                base_commit: None,
                created_at: None,
                checkpoints: Vec::new(),
            })
            .into_response();
        }
        Err(error) => return workspace_error_response(error),
    };
    match tokio::task::spawn_blocking(move || {
        let metadata = read_metadata(&root)?;
        let checkpoints = list_checkpoints_blocking(&metadata)?;
        Ok::<_, SafeWorkspaceError>(SafeWorkspaceStatus {
            enabled: true,
            workspace_path: Some(metadata.workspace_path.to_string_lossy().to_string()),
            source_path: Some(
                metadata
                    .source_repo
                    .join(&metadata.source_subpath)
                    .to_string_lossy()
                    .to_string(),
            ),
            base_commit: Some(metadata.base_commit),
            created_at: Some(metadata.created_at),
            checkpoints,
        })
    })
    .await
    {
        Ok(Ok(status)) => Json(status).into_response(),
        Ok(Err(error)) => workspace_error_response(error),
        Err(error) => workspace_error_response(SafeWorkspaceError::Io(std::io::Error::other(
            format!("safe workspace task failed: {error}"),
        ))),
    }
}

pub(in crate::gateway) async fn safe_workspace_diff_handler(
    State(state): State<RouterState>,
    AxumPath((workspace_key, session_id, checkpoint_id)): AxumPath<(String, String, String)>,
) -> Response {
    if let Err(error) = validate_key(&workspace_key, "workspace key")
        .and_then(|_| validate_key(&session_id, "session id"))
    {
        return workspace_error_response(error);
    }
    let root = match find_existing_session_root(&state.config.data_dir, &workspace_key, &session_id)
    {
        Ok(Some(root)) => root,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(error) => return workspace_error_response(error),
    };
    match tokio::task::spawn_blocking(move || {
        let _lock = WorkspaceLock::acquire(&root)?;
        let metadata = read_metadata(&root)?;
        checkpoint_diff_blocking(&metadata, &checkpoint_id)
    })
    .await
    {
        Ok(Ok(diff)) => Json(diff).into_response(),
        Ok(Err(error)) => workspace_error_response(error),
        Err(error) => workspace_error_response(SafeWorkspaceError::Io(std::io::Error::other(
            format!("safe workspace task failed: {error}"),
        ))),
    }
}

pub(in crate::gateway) async fn restore_safe_workspace_handler(
    State(state): State<RouterState>,
    AxumPath((workspace_key, session_id, checkpoint_id)): AxumPath<(String, String, String)>,
) -> Response {
    if let Err(error) = validate_key(&workspace_key, "workspace key")
        .and_then(|_| validate_key(&session_id, "session id"))
    {
        return workspace_error_response(error);
    }
    let root = match find_existing_session_root(&state.config.data_dir, &workspace_key, &session_id)
    {
        Ok(Some(root)) => root,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(error) => return workspace_error_response(error),
    };
    match tokio::task::spawn_blocking(move || restore_checkpoint_blocking(&root, &checkpoint_id))
        .await
    {
        Ok(Ok(result)) => Json(result).into_response(),
        Ok(Err(error)) => workspace_error_response(error),
        Err(error) => workspace_error_response(SafeWorkspaceError::Io(std::io::Error::other(
            format!("safe workspace task failed: {error}"),
        ))),
    }
}

pub(in crate::gateway) async fn apply_safe_workspace_handler(
    State(state): State<RouterState>,
    AxumPath((workspace_key, session_id)): AxumPath<(String, String)>,
) -> Response {
    if let Err(error) = validate_key(&workspace_key, "workspace key")
        .and_then(|_| validate_key(&session_id, "session id"))
    {
        return workspace_error_response(error);
    }
    let root = match find_existing_session_root(&state.config.data_dir, &workspace_key, &session_id)
    {
        Ok(Some(root)) => root,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(error) => return workspace_error_response(error),
    };
    match tokio::task::spawn_blocking(move || apply_to_source_blocking(&root)).await {
        Ok(Ok(result)) => Json(result).into_response(),
        Ok(Err(error)) => workspace_error_response(error),
        Err(error) => workspace_error_response(SafeWorkspaceError::Io(std::io::Error::other(
            format!("safe workspace task failed: {error}"),
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(cwd: &Path, args: &[&str]) {
        run_git(cwd, args).expect("git command should succeed");
    }

    fn fixture() -> (tempfile::TempDir, tempfile::TempDir, WorkspaceMetadata) {
        let source = tempfile::tempdir().expect("source tempdir");
        git(source.path(), &["init"]);
        git(source.path(), &["config", "user.name", "Aura Test"]);
        git(
            source.path(),
            &["config", "user.email", "aura@test.invalid"],
        );
        fs::write(source.path().join("tracked.txt"), "baseline\n").unwrap();
        git(source.path(), &["add", "tracked.txt"]);
        git(source.path(), &["commit", "-m", "baseline"]);
        fs::write(source.path().join("tracked.txt"), "dirty source\n").unwrap();
        fs::write(source.path().join("untracked.txt"), "copied\n").unwrap();

        let data = tempfile::tempdir().expect("data tempdir");
        let metadata =
            prepare_workspace_blocking(data.path(), "project-id", "session-id", source.path())
                .expect("prepare safe workspace");
        (source, data, metadata)
    }

    #[test]
    fn initializes_an_imported_non_git_workspace_without_losing_files() {
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("app.txt"), "imported\n").unwrap();
        let data = tempfile::tempdir().unwrap();

        let metadata =
            prepare_workspace_blocking(data.path(), "project-id", "session-id", source.path())
                .expect("prepare imported workspace");

        assert_eq!(
            fs::read_to_string(metadata.workspace_root.join("app.txt")).unwrap(),
            "imported\n"
        );
        assert!(source.path().join(".git").is_dir());
        assert_eq!(
            fs::read_to_string(source.path().join("app.txt")).unwrap(),
            "imported\n"
        );
    }

    #[test]
    fn restore_is_exact_and_apply_is_incremental() {
        let (source, data, metadata) = fixture();
        let baseline = list_checkpoints_blocking(&metadata).unwrap()[0].id.clone();
        fs::write(metadata.workspace_root.join("tracked.txt"), "first edit\n").unwrap();
        let root = session_root(data.path(), "project-id", "session-id");
        let first = apply_to_source_blocking(&root).unwrap();
        assert!(first.applied);
        assert_eq!(
            fs::read_to_string(source.path().join("tracked.txt")).unwrap(),
            "first edit\n"
        );

        let checkpoint = take_checkpoint(&metadata, "before bad edit").unwrap();
        fs::write(metadata.workspace_root.join("tracked.txt"), "bad edit\n").unwrap();
        let restored = restore_checkpoint_blocking(&root, &checkpoint).unwrap();
        assert!(!restored.undo_checkpoint_id.is_empty());
        assert_eq!(
            fs::read_to_string(metadata.workspace_root.join("tracked.txt")).unwrap(),
            "first edit\n"
        );

        fs::write(metadata.workspace_root.join("tracked.txt"), "second edit\n").unwrap();
        let second = apply_to_source_blocking(&root).unwrap();
        assert!(second.applied);
        assert_eq!(
            fs::read_to_string(source.path().join("tracked.txt")).unwrap(),
            "second edit\n"
        );
        assert!(!baseline.is_empty());
    }
}
