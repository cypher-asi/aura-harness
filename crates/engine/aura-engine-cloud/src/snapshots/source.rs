//! The commit a project snapshot builds from ([`delta::build_source`]),
//! and remembering a build that failed at it, so later starts don't each
//! build the same broken commit on a builder of their own.

use std::path::Path;

use super::{project_fingerprint_at, SnapshotBuilder, SnapshotRecord};
use crate::boot::{BootReporter, BootStage};
use crate::delta::{self, BuildSource};
use crate::error::{CloudError, CloudResult};
use crate::exec_errors::is_transient;

/// A build that failed for the code, at a commit and fingerprint.
#[derive(Debug, Clone)]
pub(super) struct FailedBuild {
    fingerprint: String,
    commit: String,
    error: String,
}

impl SnapshotBuilder {
    /// Where `project_dir`'s snapshot builds from: its merge target on
    /// `origin`. Says so on `boot`, with a warning when the hub's checkout
    /// has commits it leaves out or it falls back to HEAD.
    pub(super) async fn build_from(
        &self,
        project_dir: &Path,
        boot: &BootReporter,
    ) -> CloudResult<BuildSource> {
        let target = aura_exec_merge_queue::resolve_target(project_dir).await;
        let dir = project_dir.to_path_buf();
        let source = boot
            .wait(
                BootStage::Snapshot,
                format!("fetching origin/{target} to build from"),
                None,
                tokio::task::spawn_blocking(move || delta::build_source(&dir, &target)),
            )
            .await
            .map_err(|err| CloudError::io("find the build commit", std::io::Error::other(err)))??;
        let short: String = source.commit.chars().take(12).collect();
        let from = source
            .upstream
            .as_deref()
            .and_then(|name| name.strip_prefix("refs/remotes/"))
            .unwrap_or("HEAD");
        boot.fact("builds from", format!("{from} at {short}"));
        if let Some(note) = &source.note {
            tracing::warn!(project = %project_dir.display(), %note, "snapshot source");
            boot.fact("warning", note.clone());
        }
        Ok(source)
    }

    /// Refuse to build `project_id` again at `commit` and `fingerprint`
    /// when its last build there failed for the code: it would fail the
    /// same way. A `force`d build tries anyway.
    pub(super) fn refuse_failed_again(
        &self,
        project_id: &str,
        fingerprint: &str,
        commit: &str,
        force: bool,
    ) -> CloudResult<()> {
        if force {
            return Ok(());
        }
        let failed = self.failed.lock().get(project_id).cloned();
        match failed {
            Some(failed) if failed.fingerprint == fingerprint && failed.commit == commit => {
                Err(CloudError::BuildFailedBefore {
                    commit: commit.chars().take(12).collect(),
                    error: failed.error,
                })
            }
            _ => Ok(()),
        }
    }

    /// Remember how `project_id`'s build at `commit` and `fingerprint`
    /// ended: a failure for the code is refused next time
    /// ([`Self::refuse_failed_again`]); a success, or a failure of the
    /// machine, forgets it.
    pub(super) fn remember_build(
        &self,
        project_id: &str,
        (fingerprint, commit): (&str, &str),
        outcome: &CloudResult<SnapshotRecord>,
    ) {
        let mut failed = self.failed.lock();
        match outcome {
            Err(err) if !is_transient(err) => {
                failed.insert(
                    project_id.to_string(),
                    FailedBuild {
                        fingerprint: fingerprint.to_string(),
                        commit: commit.to_string(),
                        error: err.to_string(),
                    },
                );
            }
            _ => {
                failed.remove(project_id);
            }
        }
    }
}

/// [`project_fingerprint_at`], read off the async runtime.
pub(super) async fn fingerprint_at(
    dir: &Path,
    commit: &str,
    base: &str,
    command: &str,
) -> CloudResult<String> {
    let dir = dir.to_path_buf();
    let (commit, base, command) = (commit.to_string(), base.to_string(), command.to_string());
    tokio::task::spawn_blocking(move || project_fingerprint_at(&dir, &commit, &base, &command))
        .await
        .map_err(|err| CloudError::io("fingerprint project", std::io::Error::other(err)))
}
