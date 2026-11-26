//! The REPO tab's requests: the git repo of the agent on screen read into
//! a [`RepoView`], and a row's diff posted in its chat.
//!
//! Every git call is read-only, through [`aura_agent::git::read_only`],
//! and runs in a job of its own, off the event loop. PRs come from GitHub
//! a minute at a time, so the tab's refresh every few seconds doesn't ask
//! it each time.
//!
//! - `parse`: what the git calls print
//! - `load`: the view
//! - `open`: a row's diff
//! - `github`: the repo's PRs

mod github;
mod load;
mod open;
mod parse;
#[cfg(test)]
mod tests;

use super::agent_events::AgentUi;
use super::subagents::lock;
use super::LoopState;
use aura_agent::git::read_only;
use aura_core_types::AgentId;
use aura_ui_terminal::repo::{RepoRequest, RepoView};
use aura_ui_terminal::UiCommand;
use github::{Pr, Slug, Source};
use parse::{Moved, RemoteMove};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;

/// Said when the agent has no folder to read.
const NO_FOLDER: &str = "This agent has no folder yet.";

/// How long a PR list is kept before GitHub is asked again.
const PRS_FOR: Duration = Duration::from_secs(60);

/// Most entries each cache keeps; past it, it starts over.
const CACHE_CAP: usize = 4096;

/// A request from the REPO tab showing `agent_id`'s repo.
pub(super) fn handle(state: &LoopState<'_>, agent_id: AgentId, request: RepoRequest) {
    let folder = state
        .kernel_for(agent_id)
        .map(|kernel| kernel.agent_workspace())
        .or_else(|| lock(&state.hub.registry).project_folder(agent_id));
    let commands = state.commands.clone();
    tokio::spawn(async move {
        match request {
            RepoRequest::Load { force } => {
                let Some(folder) = folder else {
                    let view = RepoView {
                        agent_id: agent_id.to_hex(),
                        problem: Some(NO_FOLDER.to_string()),
                        ..RepoView::default()
                    };
                    let _ = commands.send(UiCommand::SetRepo(Box::new(view))).await;
                    return;
                };
                let (view, reading) = load::view(&folder, agent_id, force).await;
                let _ = commands.send(UiCommand::SetRepo(Box::new(view))).await;
                // The PRs this load asked for, once they come.
                if let Some(reading) = reading {
                    if reading.await.is_ok() {
                        let (view, _) = load::view(&folder, agent_id, false).await;
                        let _ = commands.send(UiCommand::SetRepo(Box::new(view))).await;
                    }
                }
            }
            RepoRequest::Open { target, title } => {
                let command = match folder {
                    Some(folder) => open::command(&folder, target, &title).await,
                    None => UiCommand::ShowWarning(NO_FOLDER.to_string()),
                };
                AgentUi::new(commands, agent_id).send(command).await;
            }
        }
    });
}

/// `git <args>` in `dir`, read-only.
async fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    read_only(dir, args).await
}

/// The GitHub repo of `root`'s branch: its upstream's remote, else
/// `origin`, when that is on github.com.
async fn github_repo(root: &Path, remote: Option<&str>) -> Option<Slug> {
    let remote = remote.unwrap_or("origin");
    if remote.starts_with('-') {
        return None;
    }
    let url = git(root, &["remote", "get-url", remote]).await.ok()?;
    github::slug(&url)
}

/// What loads keep between them.
#[derive(Default)]
struct Cache {
    /// Commits in `from..to`, by `(from, to)`
    counts: Mutex<HashMap<(String, String), usize>>,
    /// A remote-tracking branch's moves, by repo and branch: a reflog only
    /// grows when its branch moves
    remotes: Mutex<HashMap<(PathBuf, String), RemoteMoves>>,
    /// The PRs of each GitHub repo
    prs: Mutex<HashMap<Slug, Prs>>,
}

/// A remote-tracking branch's moves, with the tip they were read at.
type RemoteMoves = (String, Vec<Moved<RemoteMove>>);

static CACHE: LazyLock<Cache> = LazyLock::new(Cache::default);

/// A GitHub repo's PRs, as last read.
#[derive(Default)]
struct Prs {
    /// When they were asked for
    asked: Option<Instant>,
    /// GitHub's answer, once there is one
    answer: Option<Result<Vec<Pr>, String>>,
    /// Whether a request is out
    busy: bool,
}

/// `map` with `key` set to `value`, emptied first when it is full.
fn keep<K: std::hash::Hash + Eq, V>(map: &mut HashMap<K, V>, key: K, value: V) {
    if map.len() >= CACHE_CAP {
        map.clear();
    }
    map.insert(key, value);
}

/// `slug`'s PRs as last read from `sources`. When they are a minute old
/// they are asked for again, in a job whose handle comes back with them;
/// `force` asks at once and waits for the answer.
async fn prs(
    slug: &Slug,
    force: bool,
    sources: fn() -> Vec<Box<dyn Source>>,
) -> (Option<Result<Vec<Pr>, String>>, Option<JoinHandle<()>>) {
    if force {
        let answer = github::list(&sources(), slug).await;
        store_prs(slug, answer.clone());
        return (Some(answer), None);
    }
    let (answer, ask) = {
        let mut prs = lock(&CACHE.prs);
        if prs.len() >= CACHE_CAP {
            prs.clear();
        }
        let entry = prs.entry(slug.clone()).or_default();
        let ask = !entry.busy && entry.asked.is_none_or(|at| at.elapsed() >= PRS_FOR);
        if ask {
            entry.busy = true;
            entry.asked = Some(Instant::now());
        }
        (entry.answer.clone(), ask)
    };
    let reading = ask.then(|| {
        let slug = slug.clone();
        tokio::spawn(async move {
            let answer = github::list(&sources(), &slug).await;
            store_prs(&slug, answer);
        })
    });
    (answer, reading)
}

fn store_prs(slug: &Slug, answer: Result<Vec<Pr>, String>) {
    let mut prs = lock(&CACHE.prs);
    let entry = prs.entry(slug.clone()).or_default();
    entry.answer = Some(answer);
    entry.asked = Some(Instant::now());
    entry.busy = false;
}
