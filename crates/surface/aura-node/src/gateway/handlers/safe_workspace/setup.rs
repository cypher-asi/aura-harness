//! First-time Safe Workspace setup (source repository bootstrap, worktree
//! creation, dirty-state copy) and project-wide teardown.

use super::checkpoints::take_checkpoint;
use super::git::{run_git, run_git_allow_failure, run_git_with_input, stdout_trimmed};
use super::storage::{
    read_metadata, remove_managed_entry, session_root, source_lock_root, validate_key,
    validate_relative_path, write_metadata, WorkspaceLock,
};
use super::{
    SafeWorkspaceError, WorkspaceMetadata, CHECKPOINT_INDEX_FILE, CHECKPOINT_STORE_DIR,
    MAX_UNTRACKED_COPY_BYTES, METADATA_FILE, SAFE_WORKSPACES_DIR, WORKTREE_DIR,
};
use chrono::Utc;
use std::fs;
use std::path::{Path, PathBuf};

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
        let _ = run_git_allow_failure(
            source_repo,
            &["worktree", "remove", "--force", &workspace_arg],
        )?;
        remove_managed_entry(workspace_root)?;
    }
    run_git(source_repo, &["worktree", "prune"])?;
    remove_managed_entry(&root.join(CHECKPOINT_STORE_DIR))?;
    remove_managed_entry(&root.join(CHECKPOINT_INDEX_FILE))?;
    remove_managed_entry(&root.join(format!("{METADATA_FILE}.tmp")))?;
    Ok(())
}

/// Load an already prepared workspace and confirm it still belongs here.
fn existing_workspace(
    root: &Path,
    project_id: &str,
    session_id: &str,
) -> Result<WorkspaceMetadata, SafeWorkspaceError> {
    let metadata = read_metadata(root)?;
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
    Ok(metadata)
}

/// Returns `(source_repo, source_subpath)` for the project folder, making it
/// a Git repository first when needed.
fn resolve_source_repo(
    source_path: &Path,
    root: &Path,
) -> Result<(PathBuf, PathBuf), SafeWorkspaceError> {
    ensure_source_git_repository(source_path)?;
    let source_path = source_path.canonicalize().map_err(|error| {
        SafeWorkspaceError::Unsupported(format!("{} ({error})", source_path.display()))
    })?;
    let source_repo = PathBuf::from(stdout_trimmed(&run_git(
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
    Ok((source_repo, source_subpath))
}

/// Create the detached worktree at `HEAD` and carry over the source's
/// uncommitted and untracked changes. Returns `(base_commit, workspace_root)`.
fn create_worktree(
    source_repo: &Path,
    root: &Path,
) -> Result<(String, PathBuf), SafeWorkspaceError> {
    let base_commit = stdout_trimmed(&run_git(source_repo, &["rev-parse", "HEAD"])?);
    let workspace_root = root.join(WORKTREE_DIR);
    clean_incomplete_workspace(source_repo, root, &workspace_root)?;
    let workspace_arg = workspace_root.to_string_lossy().to_string();
    run_git(
        source_repo,
        &["worktree", "add", "--detach", &workspace_arg, &base_commit],
    )?;

    let patch = run_git(
        source_repo,
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
    copy_untracked_files(source_repo, &workspace_root)?;
    Ok((base_commit, workspace_root))
}

pub(super) fn prepare_workspace_blocking(
    data_dir: &Path,
    project_id: &str,
    session_id: &str,
    source_path: &Path,
) -> Result<WorkspaceMetadata, SafeWorkspaceError> {
    let root = session_root(data_dir, project_id, session_id);
    let _lock = WorkspaceLock::acquire(&root)?;
    if root.join(METADATA_FILE).exists() {
        return existing_workspace(&root, project_id, session_id);
    }

    // Two sessions may opt in at the same moment. Serialise the one-time Git
    // bootstrap for browser-imported projects and source worktree mutation
    // across all sessions belonging to this project.
    let _source_lock = WorkspaceLock::acquire(&source_lock_root(data_dir, project_id))?;
    let (source_repo, source_subpath) = resolve_source_repo(source_path, &root)?;
    let (base_commit, workspace_root) = create_worktree(&source_repo, &root)?;

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
    let probe = run_git_allow_failure(&canonical_source, &["rev-parse", "--show-toplevel"])?;
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

    let head = run_git_allow_failure(&canonical_source, &["rev-parse", "--verify", "HEAD"])?;
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
