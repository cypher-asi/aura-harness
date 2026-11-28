//! `git_ship`'s changelog guard: in a repo that keeps changelog
//! fragments (`changelog.d/`), a branch adds its entry there instead of
//! editing `CHANGELOG.md`, where every parallel branch's entry collided.
//!
//! The guard runs after the pull, so the diff from the fetched target
//! (`FETCH_HEAD`) to `HEAD` is exactly what the branch changes. It stops
//! the ship, before anything is pushed, when that diff edits
//! `CHANGELOG.md` and deletes no fragment. Deleting fragments while
//! editing `CHANGELOG.md` is a release (`aura changelog --write`), which
//! goes through. A repo without `changelog.d/` ships as before.

use std::path::Path;
use std::time::Duration;

use aura_exec_merge_queue::changelog::{
    fragment_name, is_fragment_path, CHANGELOG_FILE, FRAGMENT_DIR,
};
use tracing::info;

use super::executor::spawn_git;
use super::GitToolError;

/// Fail when `branch`'s change (`FETCH_HEAD..HEAD`) edits the changelog
/// of a repo that keeps fragments, instead of adding one.
pub(super) async fn check_changelog(
    workspace: &Path,
    branch: &str,
    timeout: Duration,
) -> Result<(), GitToolError> {
    let keeps_fragments = tokio::fs::metadata(workspace.join(FRAGMENT_DIR))
        .await
        .is_ok_and(|meta| meta.is_dir());
    if !keeps_fragments {
        return Ok(());
    }
    let diffed = spawn_git(
        workspace,
        "diff",
        &["--name-status", "--no-renames", "FETCH_HEAD", "HEAD"],
        timeout,
        "diff --name-status",
    )
    .await?;
    if !diffed.status.success() {
        return Err(GitToolError::NonZeroExit {
            op: "diff --name-status",
            exit_code: diffed.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&diffed.stderr).trim().to_string(),
        });
    }
    if edits_changelog_directly(&String::from_utf8_lossy(&diffed.stdout)) {
        info!(%branch, "the branch edits the changelog instead of adding a fragment; not shipped");
        return Err(GitToolError::ChangelogEdited {
            fragment: format!("{FRAGMENT_DIR}/{}", fragment_name(branch)),
        });
    }
    Ok(())
}

/// Whether a `diff --name-status` listing changes `CHANGELOG.md` without
/// deleting any fragment.
fn edits_changelog_directly(listing: &str) -> bool {
    let mut edits = false;
    let mut releases = false;
    for line in listing.lines() {
        let mut fields = line.split('\t');
        let (Some(status), Some(path)) = (fields.next(), fields.next()) else {
            continue;
        };
        if path == CHANGELOG_FILE {
            edits = true;
        } else if status.starts_with('D') && is_fragment_path(path) {
            releases = true;
        }
    }
    edits && !releases
}

#[cfg(test)]
mod tests {
    use super::edits_changelog_directly;

    #[test]
    fn only_an_edit_that_is_not_a_release_is_refused() {
        for (listing, refused) in [
            ("M\tCHANGELOG.md\nA\tsrc/a.rs", true),
            ("A\tchangelog.d/fix.md\nM\tsrc/a.rs", false),
            ("M\tCHANGELOG.md\nD\tchangelog.d/fix.md", false),
            ("M\tCHANGELOG.md\nD\tchangelog.d/README.md", true),
            ("M\tdocs/CHANGELOG.md", false),
            ("", false),
        ] {
            assert_eq!(edits_changelog_directly(listing), refused, "{listing}");
        }
    }
}
