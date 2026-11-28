//! Bringing a cloud machine's workspace up to the latest code on GitHub.
//!
//! A machine starts from its project's snapshot and the hub's HEAD
//! ([`super::start_branch`]), or from its parent's disk
//! ([`super::fork_branch`]). Either can be behind GitHub: the merge
//! queue lands other agents' work on `origin`'s main, not in the hub's
//! checkout. So before an agent works, its machine fetches
//! `origin/<target>`, the queue's target the hub resolved
//! (`aura_exec_merge_queue::resolve_target`), and merges it into the
//! agent's branch ([`pull_latest`]).
//!
//! ## Invariants
//!
//! - The pull never fails a placement and never loses work. Without
//!   `origin`, without the target branch or without access it is skipped.
//!   A merge that would touch uncommitted changes is refused by git before
//!   it changes anything; one that conflicts is aborted. Each case leaves
//!   the branch as it was and says why on one `aura-pull:` line.
//! - It only fetches into `refs/remotes/origin/<target>` and merges into
//!   the current branch; nothing is pushed.
//! - The fetch is bounded by [`FETCH_TIMEOUT_SECS`] when the machine has
//!   `timeout`, and the whole script by its exec timeout.

use super::{branch, BASE_IDENT};
use crate::error::CloudResult;

/// Branch a new machine pulls from `origin` when its project's merge
/// queue names no other target.
pub const PULL_BRANCH: &str = "main";
/// Prefix of the line [`pull_latest`] prints.
pub const PULLED: &str = "aura-pull: ";
/// Longest a machine's fetch from GitHub may take.
pub const FETCH_TIMEOUT_SECS: u32 = 120;
/// Where the pull keeps git's output for its one-line answer.
const LOG: &str = "/tmp/aura-pull.log";

/// Shell, run in the workspace, that fetches `origin/<target>` and merges
/// it into the current branch, or says why it didn't. Prints one
/// [`PULLED`] line and always exits 0.
///
/// # Errors
///
/// When `target` is not a safe branch name.
pub fn pull_latest(target: &str) -> CloudResult<String> {
    let target = branch(target)?;
    let remote = format!("refs/remotes/origin/{target}");
    Ok(format!(
        "export GIT_TERMINAL_PROMPT=0; \
         to=''; if command -v timeout >/dev/null 2>&1; then to='timeout {FETCH_TIMEOUT_SECS}'; fi; \
         ident=''; if [ -z \"$(git config user.email || true)\" ]; then ident='{BASE_IDENT}'; fi; \
         if ! git remote get-url origin >/dev/null 2>&1; then echo '{PULLED}skipped: no origin'; \
         elif ! $to git fetch -q --no-tags origin '+refs/heads/{target}:{remote}' >{LOG} 2>&1; then \
         echo \"{PULLED}skipped: could not fetch origin/{target}: $(tail -c 300 {LOG} | tr '\\n' ' ')\"; \
         elif git merge-base --is-ancestor '{remote}' HEAD; then \
         echo '{PULLED}up to date with origin/{target}'; \
         elif git $ident merge -q --no-edit '{remote}' >{LOG} 2>&1; then \
         echo \"{PULLED}merged origin/{target} at $(git rev-parse --short '{remote}')\"; \
         else if git rev-parse -q --verify MERGE_HEAD >/dev/null; then git merge --abort; fi; \
         echo \"{PULLED}skipped: origin/{target} does not merge cleanly: $(tail -c 300 {LOG} | tr '\\n' ' ')\"; \
         fi; rm -f {LOG}; "
    ))
}

/// Shell, run in the workspace, that fetches `origin/<target>` and moves
/// the branch `name`, already checked out, to it, dropping what the
/// workspace had on top: a channel member starts from what is shipped,
/// not from the hub's checkout. Prints one [`PULLED`] line and always
/// exits 0; when the fetch fails the branch stays where it was.
///
/// # Errors
///
/// When `target` or `name` is not a safe branch name.
pub fn start_from_latest(target: &str, name: &str) -> CloudResult<String> {
    let target = branch(target)?;
    let name = branch(name)?;
    let remote = format!("refs/remotes/origin/{target}");
    Ok(format!(
        "export GIT_TERMINAL_PROMPT=0; \
         to=''; if command -v timeout >/dev/null 2>&1; then to='timeout {FETCH_TIMEOUT_SECS}'; fi; \
         if ! git remote get-url origin >/dev/null 2>&1; then echo '{PULLED}skipped: no origin'; \
         elif ! $to git fetch -q --no-tags origin '+refs/heads/{target}:{remote}' >{LOG} 2>&1; then \
         echo \"{PULLED}skipped: could not fetch origin/{target}: $(tail -c 300 {LOG} | tr '\\n' ' ')\"; \
         else git checkout -q -f -B '{name}' '{remote}'; git clean -q -fd; \
         echo \"{PULLED}started from origin/{target} at $(git rev-parse --short '{remote}')\"; \
         fi; rm -f {LOG}; "
    ))
}

/// What [`pull_latest`] reported, from a script's output.
#[must_use]
pub fn parse_pulled(stdout: &str) -> Option<&str> {
    stdout
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix(PULLED))
        .map(str::trim)
}

#[cfg(test)]
mod tests;
