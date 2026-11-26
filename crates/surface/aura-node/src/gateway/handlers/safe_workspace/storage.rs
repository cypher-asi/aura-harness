//! On-disk layout of managed Safe Workspaces: session folders, lock files,
//! containment checks, metadata persistence, and key validation.

use super::{
    SafeWorkspaceError, WorkspaceMetadata, LOCK_ATTEMPTS, LOCK_RETRY_DELAY, METADATA_FILE,
    SAFE_WORKSPACES_DIR, STALE_LOCK_AGE, WORKTREE_DIR,
};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::thread;
use std::time::SystemTime;

pub(super) struct WorkspaceLock {
    path: PathBuf,
}

impl WorkspaceLock {
    pub(super) fn acquire(root: &Path) -> Result<Self, SafeWorkspaceError> {
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

pub(super) fn session_root(data_dir: &Path, project_id: &str, session_id: &str) -> PathBuf {
    data_dir
        .join(SAFE_WORKSPACES_DIR)
        .join(project_id)
        .join(session_id)
}

pub(super) fn source_lock_root(data_dir: &Path, project_id: &str) -> PathBuf {
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
pub(super) fn find_existing_session_root(
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

pub(super) fn validate_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

pub(super) fn read_metadata(root: &Path) -> Result<WorkspaceMetadata, SafeWorkspaceError> {
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

pub(super) fn write_metadata(
    root: &Path,
    metadata: &WorkspaceMetadata,
) -> Result<(), SafeWorkspaceError> {
    let encoded = serde_json::to_vec_pretty(metadata)?;
    let temporary = root.join(format!("{METADATA_FILE}.tmp"));
    fs::write(&temporary, encoded)?;
    fs::rename(temporary, root.join(METADATA_FILE))?;
    Ok(())
}

pub(super) fn remove_managed_entry(path: &Path) -> Result<(), SafeWorkspaceError> {
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

pub(super) fn validate_key<'a>(value: &'a str, label: &str) -> Result<&'a str, SafeWorkspaceError> {
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
