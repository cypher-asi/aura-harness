//! One branch per task: the names an agent's branches take, which of
//! them a pinned machine may push, and starting a task's branch.
//!
//! An agent's branches are `aura/<task-slug>-<code>`, where `<code>` is
//! its code (`rus207`). A cloud machine is pinned to the branch the hub
//! placed it on ([`crate::ToolConfig::push_branch`]); it may push that
//! branch and any other `aura/<slug>-<code>` with the same code, so it
//! can start a fresh branch for each task, but never the target or
//! another agent's branch.
//!
//! Invariants:
//! - A started branch never reuses a name that exists locally or on
//!   `origin`: `-2`, `-3`, ... are added until one is free.
//! - Starting a branch never leaves work behind: uncommitted changes, or
//!   commits `origin` has nowhere, refuse it.

use std::path::Path;
use std::time::Duration;

use super::executor::spawn_git;
use super::GitToolError;

/// Longest slug a branch takes from its task.
const SLUG_CHARS: usize = 40;
/// Most suffixes tried for a free branch name.
const MAX_SUFFIX: u32 = 99;

/// The agent code of an agent branch `aura/<slug>-<code>`: its last
/// `-` part. `None` for any other branch.
#[must_use]
pub fn agent_code(branch: &str) -> Option<&str> {
    let rest = branch.strip_prefix("aura/")?;
    let (slug, code) = rest.rsplit_once('-')?;
    let fine = !slug.is_empty()
        && !code.is_empty()
        && code.bytes().all(|b| b.is_ascii_alphanumeric())
        && !rest.contains('/');
    fine.then_some(code)
}

/// Whether a machine pinned to `pin` may push `branch`: the pin itself,
/// or another branch of the same agent.
#[must_use]
pub fn allowed_by_pin(branch: &str, pin: &str) -> bool {
    branch == pin || agent_code(pin).is_some_and(|code| agent_code(branch) == Some(code))
}

/// `text` as a branch slug: lower-case words of letters and digits,
/// joined by `-`, at most [`SLUG_CHARS`] long; `task` when nothing is
/// left.
#[must_use]
pub fn slug(text: &str) -> String {
    let mut slug = String::new();
    for word in text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        let word = word.to_ascii_lowercase();
        let extra = usize::from(!slug.is_empty());
        if slug.len() + extra + word.len() > SLUG_CHARS {
            break;
        }
        if extra == 1 {
            slug.push('-');
        }
        slug.push_str(&word);
    }
    if slug.is_empty() {
        "task".to_string()
    } else {
        slug
    }
}

/// The branch of task `task` for the agent with `code`.
#[must_use]
pub fn task_branch(task: &str, code: &str) -> String {
    format!("aura/{}-{code}", slug(task))
}

/// `wanted`, or `<slug>-2-<code>`, `-3`, ...: the first name `taken`
/// says is free.
pub(super) fn first_free(wanted: &str, taken: impl Fn(&str) -> bool) -> Option<String> {
    if !taken(wanted) {
        return Some(wanted.to_string());
    }
    let (stem, code) = match agent_code(wanted) {
        Some(code) => (&wanted[..wanted.len() - code.len() - 1], Some(code)),
        None => (wanted, None),
    };
    (2..=MAX_SUFFIX)
        .map(|n| match code {
            Some(code) => format!("{stem}-{n}-{code}"),
            None => format!("{stem}-{n}"),
        })
        .find(|name| !taken(name))
}

/// What [`start_task_branch`] made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    /// The new branch, checked out
    pub branch: String,
    /// The commit it starts at
    pub base: String,
    /// Where that commit came from (`origin/main`, or `main` without
    /// `origin`)
    pub from: String,
}

/// Make `wanted` (or the first free name after it) at the latest
/// `target` and check it out. Refuses when work would be left behind.
///
/// # Errors
/// [`GitToolError::WorkLeftBehind`] with uncommitted changes or commits
/// `origin` lacks; a git error otherwise.
pub async fn start_task_branch(
    workspace: &Path,
    wanted: &str,
    target: &str,
    timeout: Duration,
) -> Result<Started, GitToolError> {
    refuse_unsaved_work(workspace, timeout).await?;
    let has_origin = git_ok(workspace, "remote", &["get-url", "origin"], timeout)
        .await
        .is_ok();
    let from = if has_origin {
        let spec = format!("+refs/heads/{target}:refs/remotes/origin/{target}");
        git_ok(workspace, "fetch", &["--no-tags", "origin", &spec], timeout).await?;
        format!("origin/{target}")
    } else {
        target.to_string()
    };
    if has_origin {
        refuse_unpushed(workspace, timeout).await?;
    }
    let mut taken = Vec::new();
    for n in 1..=MAX_SUFFIX {
        let candidate = first_free(wanted, |name| taken.iter().any(|t: &String| t == name));
        let Some(candidate) = candidate else { break };
        if branch_exists(workspace, &candidate, has_origin, timeout).await? {
            taken.push(candidate);
            continue;
        }
        let base = git_ok(workspace, "rev-parse", &["--verify", &from], timeout).await?;
        git_ok(
            workspace,
            "checkout",
            &["-q", "-b", &candidate, &from],
            timeout,
        )
        .await?;
        tracing::info!(branch = %candidate, %from, tries = n, "Started a task branch");
        return Ok(Started {
            branch: candidate,
            base,
            from,
        });
    }
    Err(GitToolError::InvalidArg {
        name: "task",
        reason: format!("no free branch name after {wanted}"),
    })
}

/// Whether `name` is a branch here or on `origin`.
async fn branch_exists(
    workspace: &Path,
    name: &str,
    on_origin: bool,
    timeout: Duration,
) -> Result<bool, GitToolError> {
    let local = format!("refs/heads/{name}");
    let here = spawn_git(
        workspace,
        "rev-parse",
        &["--verify", "--quiet", &local],
        timeout,
        "rev-parse",
    )
    .await?;
    if here.status.success() {
        return Ok(true);
    }
    if !on_origin {
        return Ok(false);
    }
    let listed = git_ok(
        workspace,
        "ls-remote",
        &["--heads", "origin", &local],
        timeout,
    )
    .await?;
    Ok(!listed.is_empty())
}

async fn refuse_unsaved_work(workspace: &Path, timeout: Duration) -> Result<(), GitToolError> {
    let status = git_ok(workspace, "status", &["--porcelain"], timeout).await?;
    if status.is_empty() {
        return Ok(());
    }
    let files: Vec<&str> = status
        .lines()
        .take(10)
        .map(|line| line.get(3..).unwrap_or(line))
        .collect();
    Err(GitToolError::WorkLeftBehind(format!(
        "uncommitted changes in {}; ship them (git_ship) or commit them first",
        files.join(", ")
    )))
}

async fn refuse_unpushed(workspace: &Path, timeout: Duration) -> Result<(), GitToolError> {
    let unpushed = git_ok(
        workspace,
        "rev-list",
        &[
            "--max-count=10",
            "--oneline",
            "HEAD",
            "--not",
            "--remotes=origin",
        ],
        timeout,
    )
    .await
    .map_err(|error| {
        GitToolError::WorkLeftBehind(format!(
            "could not tell which commits origin doesn't have ({error}), so a new branch could \
             leave them behind"
        ))
    })?;
    if unpushed.is_empty() {
        return Ok(());
    }
    Err(GitToolError::WorkLeftBehind(format!(
        "commits origin doesn't have: {}; ship them (git_ship) first",
        unpushed.lines().collect::<Vec<_>>().join("; ")
    )))
}

/// The branch follow-up work on `branch` moves to: when the branch's
/// last push (`origin/<branch>` as this clone last saw it) is already on
/// `origin/<target>` and new commits sit on top, a fresh name
/// (from `task` when given, else the old name with a number), checked
/// out at `HEAD`. `None` when the branch is not merged or has nothing
/// new, or is no agent branch. `allowed` checks the new name (the push
/// pin, protected branches) before anything is checked out, so a refused
/// name leaves the workspace where it was.
///
/// # Errors
/// A git error, or what `allowed` refuses.
pub async fn follow_up_branch(
    workspace: &Path,
    branch: &str,
    target: &str,
    task: Option<&str>,
    allowed: impl Fn(&str) -> Result<(), GitToolError>,
    timeout: Duration,
) -> Result<Option<String>, GitToolError> {
    let Some(code) = agent_code(branch) else {
        return Ok(None);
    };
    let tracking = format!("refs/remotes/origin/{branch}");
    let Ok(pushed) = git_ok(
        workspace,
        "rev-parse",
        &["--verify", "--quiet", &tracking],
        timeout,
    )
    .await
    else {
        return Ok(None);
    };
    let spec = format!("+refs/heads/{target}:refs/remotes/origin/{target}");
    git_ok(workspace, "fetch", &["--no-tags", "origin", &spec], timeout).await?;
    let landed = format!("refs/remotes/origin/{target}");
    let is_ancestor = |commit: String| {
        let landed = landed.clone();
        async move {
            spawn_git(
                workspace,
                "merge-base",
                &["--is-ancestor", &commit, &landed],
                timeout,
                "merge-base",
            )
            .await
            .map(|out| out.status.success())
        }
    };
    if !is_ancestor(pushed).await? || is_ancestor("HEAD".to_string()).await? {
        return Ok(None);
    }
    let wanted = task.map_or_else(|| branch.to_string(), |task| task_branch(task, code));
    let Some(candidate) = free_name(workspace, &wanted, branch, timeout).await? else {
        return Ok(None);
    };
    allowed(&candidate)?;
    git_ok(workspace, "checkout", &["-q", "-b", &candidate], timeout).await?;
    tracing::info!(from = %branch, to = %candidate, "Follow-up work on a merged branch moved to a new one");
    Ok(Some(candidate))
}

/// The first free name from `wanted` on, here and on origin, other than
/// `current`.
async fn free_name(
    workspace: &Path,
    wanted: &str,
    current: &str,
    timeout: Duration,
) -> Result<Option<String>, GitToolError> {
    let mut taken = vec![current.to_string()];
    while let Some(candidate) = first_free(wanted, |name| taken.iter().any(|t| t == name)) {
        if !branch_exists(workspace, &candidate, true, timeout).await? {
            return Ok(Some(candidate));
        }
        taken.push(candidate);
    }
    Ok(None)
}

/// `git <subcmd> <args>`, its trimmed standard output, failing on a
/// non-zero exit.
pub(super) async fn git_ok(
    workspace: &Path,
    subcmd: &'static str,
    args: &[&str],
    timeout: Duration,
) -> Result<String, GitToolError> {
    let out = spawn_git(workspace, subcmd, args, timeout, subcmd).await?;
    if !out.status.success() {
        return Err(GitToolError::NonZeroExit {
            op: subcmd,
            exit_code: out.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_branches_carry_their_agents_code() {
        assert_eq!(agent_code("aura/fix-scroll-rus207"), Some("rus207"));
        assert_eq!(agent_code("aura/a-b-c-fen6"), Some("fen6"));
        for not_one in [
            "main",
            "aura/rus207",
            "feature/x-rus207",
            "aura/x/y-rus1",
            "aura/-rus1",
        ] {
            assert_eq!(agent_code(not_one), None, "{not_one}");
        }
    }

    #[test]
    fn a_pinned_machine_may_push_its_agents_branches_only() {
        let pin = "aura/improve-the-software-factory-rus207";
        assert!(allowed_by_pin(pin, pin));
        assert!(allowed_by_pin("aura/queue-tab-tests-rus207", pin));
        for refused in [
            "aura/x-rus204",
            "main",
            "aura/x-rus2070",
            "feature/x-rus207",
        ] {
            assert!(!allowed_by_pin(refused, pin), "{refused}");
        }
        assert!(
            !allowed_by_pin("aura/x-rus207", "main"),
            "a pin with no code allows only itself"
        );
    }

    #[test]
    fn a_task_becomes_a_short_branch_slug() {
        assert_eq!(
            task_branch("Fix the Queue tab: tests first!", "rus207"),
            "aura/fix-the-queue-tab-tests-first-rus207"
        );
        assert_eq!(slug("   "), "task");
        assert!(slug(&"word ".repeat(30)).len() <= SLUG_CHARS);
    }

    #[test]
    fn a_taken_name_gets_the_next_number_before_the_code() {
        let taken = ["aura/x-rus1", "aura/x-2-rus1"];
        let free = first_free("aura/x-rus1", |name| taken.contains(&name));
        assert_eq!(free.as_deref(), Some("aura/x-3-rus1"));
        assert_eq!(
            first_free("aura/y-rus1", |_| false).as_deref(),
            Some("aura/y-rus1")
        );
    }
}
