//! Shadow-repository checkpoints: snapshot, list, diff, restore, and apply a
//! session's changes back to the source project.

use super::git::{run_git, run_git_with_input, run_shadow, stdout_trimmed};
use super::storage::{read_metadata, write_metadata, WorkspaceLock};
use super::{
    SafeWorkspaceApplyResult, SafeWorkspaceCheckpoint, SafeWorkspaceDiff, SafeWorkspaceError,
    SafeWorkspaceRestoreResult, WorkspaceMetadata, CHECKPOINT_INDEX_FILE, CHECKPOINT_REF,
    CHECKPOINT_STORE_DIR, MAX_CHECKPOINTS_RETURNED, MAX_DIFF_BYTES, MAX_SNAPSHOT_FILES,
};
use std::fs;
use std::path::Path;

fn initialize_shadow_store(metadata: &WorkspaceMetadata) -> Result<(), SafeWorkspaceError> {
    let root = metadata
        .workspace_root
        .parent()
        .ok_or_else(|| SafeWorkspaceError::InvalidMetadata("missing session root".to_string()))?;
    let store = root.join(CHECKPOINT_STORE_DIR);
    if !store.join("HEAD").exists() {
        fs::create_dir_all(&store)?;
        run_git(root, &["init", "--bare", store.to_string_lossy().as_ref()])?;
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
    Ok(output.status.success().then(|| stdout_trimmed(&output)))
}

/// Refuse to snapshot workspaces with more files than the checkpoint limit.
fn ensure_snapshot_size(metadata: &WorkspaceMetadata) -> Result<(), SafeWorkspaceError> {
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
    Ok(())
}

/// Stage the whole worktree into the shadow index on top of `parent` and
/// return the resulting tree id.
fn write_snapshot_tree(
    metadata: &WorkspaceMetadata,
    parent: Option<&str>,
) -> Result<String, SafeWorkspaceError> {
    let index_path = metadata.session_dir()?.join(CHECKPOINT_INDEX_FILE);
    if let Some(parent) = parent {
        run_shadow(metadata, &["read-tree", parent], false)?;
    } else if index_path.exists() {
        fs::remove_file(&index_path)?;
    }

    run_shadow(metadata, &["add", "-A", "--", "."], false)?;
    Ok(stdout_trimmed(&run_shadow(
        metadata,
        &["write-tree"],
        false,
    )?))
}

fn commit_checkpoint(
    metadata: &WorkspaceMetadata,
    tree: &str,
    parent: Option<&str>,
    reason: &str,
) -> Result<String, SafeWorkspaceError> {
    let mut args = vec!["commit-tree", tree, "-m", reason, "--no-gpg-sign"];
    if let Some(parent) = parent {
        args.splice(2..2, ["-p", parent]);
    }
    let commit = stdout_trimmed(&run_shadow(metadata, &args, false)?);
    let mut update_args = vec!["update-ref", CHECKPOINT_REF, commit.as_str()];
    if let Some(parent) = parent {
        update_args.push(parent);
    }
    run_shadow(metadata, &update_args, false)?;
    Ok(commit)
}

pub(super) fn take_checkpoint(
    metadata: &WorkspaceMetadata,
    reason: &str,
) -> Result<String, SafeWorkspaceError> {
    initialize_shadow_store(metadata)?;
    ensure_snapshot_size(metadata)?;

    let parent = checkpoint_tip(metadata)?;
    let tree = write_snapshot_tree(metadata, parent.as_deref())?;

    if let Some(parent) = parent.as_deref() {
        let parent_tree = stdout_trimmed(&run_shadow(
            metadata,
            &["rev-parse", &format!("{parent}^{{tree}}")],
            false,
        )?);
        if parent_tree == tree {
            return Ok(parent.to_string());
        }
    }

    commit_checkpoint(metadata, &tree, parent.as_deref(), reason)
}

pub(super) fn list_checkpoints_blocking(
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
    let resolved = stdout_trimmed(&resolved);
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

pub(super) fn checkpoint_diff_blocking(
    metadata: &WorkspaceMetadata,
    checkpoint_id: &str,
) -> Result<SafeWorkspaceDiff, SafeWorkspaceError> {
    let checkpoint_id = validate_checkpoint_id(metadata, checkpoint_id)?;
    run_shadow(metadata, &["add", "-A", "--", "."], false)?;
    let stat = stdout_trimmed(&run_shadow(
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

pub(super) fn restore_checkpoint_blocking(
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
    stdout_trimmed(&output)
        .lines()
        .last()
        .map(str::to_string)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| SafeWorkspaceError::InvalidMetadata("baseline checkpoint missing".into()))
}

/// The checkpoint the next handoff diffs from: the last applied one, or the
/// session baseline.
fn apply_baseline(metadata: &WorkspaceMetadata) -> Result<String, SafeWorkspaceError> {
    match metadata.applied_checkpoint.as_deref() {
        Some(id) => validate_checkpoint_id(metadata, id),
        None => first_checkpoint(metadata),
    }
}

/// Returns `(stat, patch)` between two checkpoints.
fn checkpoint_patch(
    metadata: &WorkspaceMetadata,
    baseline: &str,
    checkpoint_id: &str,
) -> Result<(String, Vec<u8>), SafeWorkspaceError> {
    let stat = stdout_trimmed(&run_shadow(
        metadata,
        &["diff", "--stat", baseline, checkpoint_id],
        false,
    )?);
    let patch = run_shadow(
        metadata,
        &["diff", "--binary", "--full-index", baseline, checkpoint_id],
        false,
    )?
    .stdout;
    Ok((stat, patch))
}

fn apply_patch_to_source(
    metadata: &WorkspaceMetadata,
    patch: &[u8],
) -> Result<(), SafeWorkspaceError> {
    if let Err(error) = run_git_with_input(
        &metadata.source_repo,
        &["apply", "--check", "--whitespace=nowarn", "-"],
        patch,
    ) {
        return Err(SafeWorkspaceError::Conflict(error.to_string()));
    }
    if let Err(error) = run_git_with_input(
        &metadata.source_repo,
        &["apply", "--whitespace=nowarn", "-"],
        patch,
    ) {
        return Err(SafeWorkspaceError::Conflict(error.to_string()));
    }
    Ok(())
}

pub(super) fn apply_to_source_blocking(
    root: &Path,
) -> Result<SafeWorkspaceApplyResult, SafeWorkspaceError> {
    let _lock = WorkspaceLock::acquire(root)?;
    let mut metadata = read_metadata(root)?;
    let project_root = root.parent().ok_or_else(|| {
        SafeWorkspaceError::InvalidMetadata("managed project directory is missing".to_string())
    })?;
    let _source_lock = WorkspaceLock::acquire(&project_root.join(".source-lock"))?;
    let checkpoint_id = take_checkpoint(&metadata, "before applying changes to project")?;
    let baseline = apply_baseline(&metadata)?;
    let (stat, patch) = checkpoint_patch(&metadata, &baseline, &checkpoint_id)?;

    if patch.is_empty() {
        return Ok(SafeWorkspaceApplyResult {
            applied: false,
            checkpoint_id,
            stat,
            source_path: metadata.source_display_path(),
        });
    }

    apply_patch_to_source(&metadata, &patch)?;

    metadata.applied_checkpoint = Some(checkpoint_id.clone());
    write_metadata(root, &metadata)?;
    Ok(SafeWorkspaceApplyResult {
        applied: true,
        checkpoint_id,
        stat,
        source_path: metadata.source_display_path(),
    })
}
