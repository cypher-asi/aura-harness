//! A workspace as it was before an attempt, and everything the attempt
//! changed since, by its file tools and by the commands it ran.

use crate::event_loop::tasks::TASKS_DIR;
use crate::isolation::changes;
use aura_core_types::CompareCopy;
use aura_gate::attempts::CopyAt;
use aura_gate::live;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// How long recording a workspace before a turn may take; a workspace
/// that takes longer is not watched.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long reading what an attempt changed may take.
const DIFF_TIMEOUT: Duration = Duration::from_secs(30);

/// Most bytes of untracked files a workspace may hold and still be
/// recorded before a turn: recording stores a copy of each in the
/// repository's objects.
const MAX_UNTRACKED_BYTES: u64 = 64 * 1024 * 1024;

/// A workspace as it was before an attempt, to read everything the
/// attempt changed from.
#[derive(Debug, Clone)]
pub(crate) enum Before {
    /// A `/compare` run's copy and the tree it started from
    Copy(CopyAt),
    /// A git working tree as it was when the turn started
    Snapshot(Arc<changes::Snapshot>),
}

/// Everything an attempt changed, by its file tools and by the commands
/// it ran.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Changed {
    /// As a unified diff
    pub diff: String,
    /// The files, relative to the root of the working tree or copy
    pub paths: Vec<String>,
}

impl Before {
    /// The copy a `/compare` run works in, as it was before the run.
    pub(crate) fn of_copy(copy: &CompareCopy) -> Self {
        Self::Copy(live::copy_at(copy))
    }

    /// The copy `copy` as it is now, part of the way through its run.
    /// `None` when git could not record it.
    pub(crate) async fn of_copy_now(copy: &CompareCopy) -> Option<Self> {
        let mut at = live::copy_at(copy);
        let (root, git_dir) = (at.root.clone(), at.git_dir.clone());
        let tree =
            tokio::task::spawn_blocking(move || changes::baseline(&root, git_dir.as_deref()));
        match tokio::time::timeout(SNAPSHOT_TIMEOUT, tree).await {
            Ok(Ok(Ok(tree))) => {
                at.baseline = tree;
                Some(Self::Copy(at))
            }
            _ => None,
        }
    }

    /// `workspace` as it is now, when it is in a git working tree that
    /// can be recorded within [`SNAPSHOT_TIMEOUT`] and holds no more
    /// than [`MAX_UNTRACKED_BYTES`] of untracked files. Only objects are
    /// written to the repository (see [`changes::snapshot`]).
    pub(crate) async fn of_workspace(workspace: PathBuf) -> Option<Self> {
        let taken = tokio::task::spawn_blocking(move || {
            if changes::untracked_bytes(&workspace)? > MAX_UNTRACKED_BYTES {
                info!(workspace = %workspace.display(), "Too many untracked bytes to record the workspace before the turn");
                return None;
            }
            match changes::snapshot(&workspace) {
                Ok(snapshot) => snapshot,
                Err(e) => {
                    warn!(workspace = %workspace.display(), error = %e, "Could not record the workspace before the turn");
                    None
                }
            }
        });
        match tokio::time::timeout(SNAPSHOT_TIMEOUT, taken).await {
            Ok(Ok(Some(snapshot))) => Some(Self::Snapshot(Arc::new(snapshot))),
            Ok(_) => None,
            Err(_) => {
                warn!("Recording the workspace before the turn took too long");
                None
            }
        }
    }

    /// Everything that changed since. `None` when git could not say
    /// within [`DIFF_TIMEOUT`].
    pub(crate) async fn changed(&self) -> Option<Changed> {
        let before = self.clone();
        let read = tokio::task::spawn_blocking(move || match before {
            Self::Copy(copy) => changes::readable_diff_since(
                &copy.root,
                copy.git_dir.as_deref(),
                &copy.baseline,
                &[TASKS_DIR],
            ),
            Self::Snapshot(snapshot) => snapshot.diff(),
        });
        match tokio::time::timeout(DIFF_TIMEOUT, read).await {
            Ok(Ok(Ok(diff))) => Some(Changed {
                paths: changes::paths_in(&diff),
                diff,
            }),
            Ok(Ok(Err(e))) => {
                warn!(error = %e, "Could not read what the attempt changed");
                None
            }
            _ => {
                warn!("Reading what the attempt changed took too long");
                None
            }
        }
    }
}
