//! The [`Snapshot`] record and the parts of a restore that need no git.

use std::path::PathBuf;

use super::{names, IsolationError};

/// Mode git gives a nested repository (a submodule or an embedded clone);
/// a restore leaves those alone.
const GITLINK: &str = "160000";

/// A folder of a git working tree as it was at one moment: its tracked
/// and untracked files git does not ignore. See [`snapshot`].
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Root of the working tree
    pub(super) root: PathBuf,
    /// The folder recorded, relative to `root` (`.` for all of it)
    pub(super) scope: PathBuf,
    /// Its files, as a git tree
    pub(super) tree: String,
    /// Untracked files and folders (ending in `/`) git ignored then,
    /// relative to `root`
    pub(super) ignored: Vec<String>,
}

/// What [`Snapshot::restore`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Restored {
    /// Files written back as they were
    pub written: Vec<PathBuf>,
    /// Files the snapshot did not have, removed
    pub removed: Vec<PathBuf>,
    /// Files not written back because a folder that still holds files
    /// (ignored ones, or a nested repository) stands where they go
    pub blocked: Vec<PathBuf>,
}

impl Snapshot {
    /// Whether `path` (relative to the root) was an ignored file, or in
    /// an ignored folder, when the snapshot was taken.
    fn was_ignored(&self, path: &str) -> bool {
        self.ignored.iter().any(|entry| {
            if entry.ends_with('/') {
                path.starts_with(entry.as_str())
            } else {
                path == entry
            }
        })
    }
}

impl Snapshot {
    /// The files in the `diff-tree --raw -z` output `raw`: those to write
    /// back from the snapshot, and those created since (ignored ones left
    /// out). Nested repositories are left alone.
    pub(super) fn sort_changes(
        &self,
        raw: &[u8],
    ) -> Result<(Vec<String>, Vec<String>), IsolationError> {
        let mut back = Vec::new();
        let mut created = Vec::new();
        let mut fields = names(raw);
        while let (Some(meta), Some(path)) = (fields.next(), fields.next()) {
            let parts: Vec<&str> = meta.trim_start_matches(':').split(' ').collect();
            let [old_mode, new_mode, _, _, status] = parts[..] else {
                return Err(IsolationError::Git(format!(
                    "git diff-tree: unexpected line `{meta}`"
                )));
            };
            if old_mode == GITLINK || new_mode == GITLINK {
                continue;
            }
            if status == "A" {
                if !self.was_ignored(&path) {
                    created.push(path);
                }
            } else {
                back.push(path);
            }
        }
        Ok((back, created))
    }

    /// Remove the files `created` since the snapshot.
    pub(super) fn remove_created(
        &self,
        created: Vec<String>,
        restored: &mut Restored,
    ) -> Result<(), IsolationError> {
        for path in created {
            let full = self.root.join(&path);
            match std::fs::remove_file(&full) {
                Ok(()) => restored.removed.push(full),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// Keep in `back` only the files nothing blocks: `checkout-index
    /// --force` deletes a whole folder standing where a file goes back,
    /// ignored files in it included; only an empty one may go.
    pub(super) fn keep_unblocked(&self, back: &mut Vec<String>, restored: &mut Restored) {
        back.retain(|path| {
            let full = self.root.join(path);
            let folder = full.symlink_metadata().is_ok_and(|meta| meta.is_dir());
            if folder && std::fs::remove_dir(&full).is_err() {
                restored.blocked.push(full);
                return false;
            }
            true
        });
    }
}
