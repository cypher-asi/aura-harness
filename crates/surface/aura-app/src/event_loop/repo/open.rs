//! What a REPO row opens: its diff, read for the chat.
//!
//! Targets come from the tab, so each is checked before git sees it:
//! shas are hex, stashes are `stash@{N}`, branches, tags and worktrees
//! are the repo's own, and paths stay inside it.

use super::super::compare::view::split_patch;
use super::super::handlers::system_message;
use super::parse::{self, is_sha, is_stash};
use super::{git, github, github_repo};
use aura_ui::events::FileDiffData;
use aura_ui::repo::RepoTarget;
use aura_ui::UiCommand;
use std::path::{Component, Path, PathBuf};

/// Most of a diff the chat is sent, in bytes; the files past it are left
/// out.
const MAX_PATCH: usize = 1 << 20;
/// Most files of a diff the chat is sent.
const MAX_FILES: usize = 300;
/// Most characters of a row's title kept for the diff's.
const MAX_TITLE: usize = 200;
/// Untracked files of a folder shown at most.
const MAX_UNTRACKED: usize = 50;

/// The chat's answer to opening `target` in `folder`'s repo: its diff
/// under `title`, or why there is none.
pub(super) async fn command(folder: &Path, target: RepoTarget, title: &str) -> UiCommand {
    let title: String = title
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(MAX_TITLE)
        .collect();
    match diff(folder, &target).await {
        Ok(patch) => {
            let files = split_patch(&patch);
            if files.is_empty() {
                return system_message(format!("{title}: nothing changed."));
            }
            let total = files.len();
            let files = cap(files);
            let title = if files.len() < total {
                format!("{title} (the first {} of {total} files)", files.len())
            } else {
                title
            };
            UiCommand::ShowDiff { title, files }
        }
        Err(problem) => UiCommand::ShowWarning(format!("{title}: {problem}")),
    }
}

/// `files` cut to [`MAX_FILES`] and [`MAX_PATCH`] bytes; a first file
/// bigger than that alone is cut at a line.
fn cap(files: Vec<FileDiffData>) -> Vec<FileDiffData> {
    let mut kept = Vec::new();
    let mut size = 0;
    for mut file in files {
        if kept.len() == MAX_FILES || (size > 0 && size + file.diff.len() > MAX_PATCH) {
            break;
        }
        if file.diff.len() > MAX_PATCH {
            let mut end = MAX_PATCH;
            while !file.diff.is_char_boundary(end) {
                end -= 1;
            }
            let cut = file.diff[..end].rfind('\n').map_or(0, |at| at + 1);
            file.diff.truncate(cut);
        }
        size += file.diff.len();
        kept.push(file);
    }
    kept
}

async fn diff(folder: &Path, target: &RepoTarget) -> Result<String, String> {
    let top = git(folder, &["rev-parse", "--show-toplevel"]).await?;
    let root = PathBuf::from(top.trim_end_matches('\n'));
    match target {
        RepoTarget::Unpushed => git(&root, &["diff", "-M", "@{upstream}...HEAD"]).await,
        RepoTarget::Change { path, from } => change(&root, path, from.as_deref()).await,
        RepoTarget::Commit { sha } => commit(&root, sha).await,
        RepoTarget::Range { from, to } => range(&root, from.as_deref(), to).await,
        RepoTarget::Stash { name } => stash(&root, name).await,
        RepoTarget::Pr { number } => pr(&root, *number).await,
        RepoTarget::Branch { name } => branch(&root, name).await,
        RepoTarget::Tag { name } => tag(&root, name).await,
        RepoTarget::Worktree { path } => worktree(&root, path).await,
    }
}

/// What commit `sha` changed, against its first parent.
async fn commit(root: &Path, sha: &str) -> Result<String, String> {
    sha_only(sha)?;
    git(
        root,
        &[
            "show",
            "--format=",
            "--patch",
            "-M",
            "--diff-merges=first-parent",
            "--end-of-options",
            sha,
        ],
    )
    .await
}

/// What changed from `from` to `to`; without `from`, since `to` left the
/// main line ([`base`]).
async fn range(root: &Path, from: Option<&str>, to: &str) -> Result<String, String> {
    sha_only(to)?;
    let from = match from {
        Some(from) => {
            sha_only(from)?;
            from.to_string()
        }
        None => base(root, to).await?,
    };
    git(root, &["diff", "-M", "--end-of-options", &from, to]).await
}

/// What the stash `name` holds, untracked files and all.
async fn stash(root: &Path, name: &str) -> Result<String, String> {
    if !is_stash(name) {
        return Err(format!("{name} is not a stash"));
    }
    git(root, &["stash", "show", "-p", "--include-untracked", name]).await
}

/// PR `number`'s diff, from GitHub.
async fn pr(root: &Path, number: u64) -> Result<String, String> {
    let Some(slug) = github_repo(root, upstream_remote(root).await.as_deref()).await else {
        return Err("this repo is not on GitHub".to_string());
    };
    github::diff(&github::sources(), &slug, number).await
}

/// What the branch `name` has that `HEAD` doesn't.
async fn branch(root: &Path, name: &str) -> Result<String, String> {
    let branch = own_ref(root, "refs/heads/", name).await?;
    git(
        root,
        &[
            "diff",
            "-M",
            "--end-of-options",
            &format!("HEAD...{branch}"),
        ],
    )
    .await
}

/// What changed from the tag `name` to `HEAD`.
async fn tag(root: &Path, name: &str) -> Result<String, String> {
    let tag = own_ref(root, "refs/tags/", name).await?;
    git(root, &["diff", "-M", "--end-of-options", &tag, "HEAD"]).await
}

/// The uncommitted changes of the repo's worktree at `path`.
async fn worktree(root: &Path, path: &Path) -> Result<String, String> {
    let listed = git(root, &["worktree", "list", "--porcelain"]).await?;
    let Some(worktree) = parse::worktrees(&listed)
        .into_iter()
        .map(|w| w.path)
        .find(|listed| listed == path)
    else {
        return Err(format!("{} is not a worktree of this repo", path.display()));
    };
    git(&worktree, &["diff", "-M", "HEAD"]).await
}

fn sha_only(sha: &str) -> Result<(), String> {
    if is_sha(sha) {
        Ok(())
    } else {
        Err(format!("{sha} is not a commit"))
    }
}

/// `name` under `prefix` (`refs/heads/`), when the repo has it.
async fn own_ref(root: &Path, prefix: &str, name: &str) -> Result<String, String> {
    let fine = !name.is_empty()
        && !name.starts_with('-')
        && !name.contains("..")
        && !name.contains("@{")
        && !name
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c));
    let full = format!("{prefix}{name}");
    if fine
        && git(root, &["rev-parse", "--verify", "--quiet", &full])
            .await
            .is_ok()
    {
        Ok(full)
    } else {
        Err(format!("no {name} here"))
    }
}

/// The remote of the branch checked out, if it has an upstream.
async fn upstream_remote(root: &Path) -> Option<String> {
    let head = git(root, &["rev-parse", "--symbolic-full-name", "HEAD"])
        .await
        .ok()?;
    let head = head.trim();
    if !head.starts_with("refs/heads/") {
        return None;
    }
    let remote = git(
        root,
        &["for-each-ref", "--format=%(upstream:remotename)", head],
    )
    .await
    .ok()?;
    let remote = remote.lines().next().unwrap_or_default().trim();
    Some(remote.to_string()).filter(|remote| !remote.is_empty())
}

/// Where `to` left the main line: its merge base with the remote's
/// default branch, else with `main` or `master`.
async fn base(root: &Path, to: &str) -> Result<String, String> {
    for main in [
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
        "refs/remotes/origin/master",
        "refs/heads/main",
        "refs/heads/master",
    ] {
        if git(root, &["rev-parse", "--verify", "--quiet", main])
            .await
            .is_err()
        {
            continue;
        }
        let base = git(root, &["merge-base", main, to]).await?;
        let base = base.trim();
        if base == to {
            return Err("nothing new: it is on the main branch".to_string());
        }
        return Ok(base.to_string());
    }
    Err("no main branch to compare it with".to_string())
}

/// One uncommitted change: an untracked file (or a folder's files)
/// against nothing, else the file against `HEAD`, with where a rename
/// came from.
async fn change(root: &Path, path: &str, from: Option<&str>) -> Result<String, String> {
    inside(path)?;
    if let Some(from) = from {
        inside(from)?;
    }
    let untracked = git(
        root,
        &[
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            path,
        ],
    )
    .await?;
    let untracked: Vec<&str> = untracked.split('\0').filter(|p| !p.is_empty()).collect();
    if !untracked.is_empty() {
        let mut patch = String::new();
        for file in untracked.into_iter().take(MAX_UNTRACKED) {
            patch.push_str(&git(root, &["diff", "--no-index", "--", "/dev/null", file]).await?);
        }
        return Ok(patch);
    }
    let born = git(root, &["rev-parse", "--verify", "--quiet", "HEAD"])
        .await
        .is_ok();
    let mut args = vec![
        "diff",
        "-M",
        if born { "HEAD" } else { "--cached" },
        "--",
        path,
    ];
    args.extend(from);
    git(root, &args).await
}

/// Whether `path` is a path inside the repo, as `status` gives them.
fn inside(path: &str) -> Result<(), String> {
    let fine = !path.is_empty()
        && Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)));
    if fine {
        Ok(())
    } else {
        Err(format!("{path} is not in the repo"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_big_diff_keeps_its_first_files() {
        let file = |n: usize, size: usize| FileDiffData {
            path: format!("{n}.txt"),
            diff: "+x\n".repeat(size / 3),
        };
        let kept = cap((0..4).map(|n| file(n, MAX_PATCH / 3)).collect());
        assert_eq!(kept.len(), 3, "the fourth goes past the cap");
        let kept = cap(vec![file(0, MAX_PATCH * 2), file(1, 10)]);
        assert_eq!(kept.len(), 1);
        assert!(kept[0].diff.len() <= MAX_PATCH && kept[0].diff.ends_with('\n'));
        assert_eq!(
            cap((0..MAX_FILES + 5).map(|n| file(n, 3)).collect()).len(),
            MAX_FILES
        );
    }

    #[test]
    fn paths_stay_in_the_repo() {
        assert!(inside("src/main.rs").is_ok());
        assert!(inside("new folder/").is_ok());
        for path in ["", "/etc/passwd", "../x", "src/../../x", "./x"] {
            assert!(inside(path).is_err(), "{path}");
        }
    }
}
