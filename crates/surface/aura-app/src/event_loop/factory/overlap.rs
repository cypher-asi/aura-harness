//! Overlap warnings: when a push is queued, the hub records which files
//! its entry changes and tells both agents when other work in flight
//! changes the same ones, so they can agree who lands first. Nothing is
//! blocked. See `docs/software-factory.md` §8 and
//! [`aura_exec_merge_queue::overlap`].
//!
//! The work in flight is the queue's open entries plus each local
//! worktree in the repo that one agent works in, on a branch other than
//! the target. A cloud member's work counts once it is pushed.
//!
//! ## Invariants
//!
//! - A pair of branches is told about the same shared files once per
//!   process ([`Told`]); a re-push with the same overlap stays quiet.
//! - The worktrees last read for a repo are kept for its QUEUE tab
//!   ([`entry_overlaps`]), so drawing the tab runs no git.

use crate::event_loop::notices::Notice;
use crate::event_loop::subagents::{lock, AgentHub};
use aura_core_types::AgentId;
use aura_exec_merge_queue::config::GIT_TIMEOUT;
use aura_exec_merge_queue::overlap::{self, Overlap, Work};
use aura_exec_merge_queue::{EntryId, MergeEntry, MergeQueue, RepoPaths};
use chrono::Utc;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tracing::{info, warn};

/// Most shared files named in one line of the QUEUE tab or a notice.
const SHOWN_FILES: usize = 5;

/// Record the files entry `id` of `queue` (merging into `target`)
/// changes, then warn its agent and the owners of the work it overlaps.
pub(super) async fn scan(hub: &AgentHub, queue: &MergeQueue, target: &str, id: &EntryId) {
    let entries = queue.entries().await;
    let Some(mut entry) = entries.iter().find(|entry| &entry.id == id).cloned() else {
        return;
    };
    let repo = queue.repo();
    match overlap::changed_files(repo, target, &entry, GIT_TIMEOUT).await {
        Ok(files) => entry.files = files,
        Err(why) => {
            warn!(entry = %entry.id, %why, "Could not read the files a queued push changes");
            return;
        }
    }
    if let Err(why) = queue
        .record_files(&entry.id, &entry.head, entry.files.clone())
        .await
    {
        warn!(entry = %entry.id, %why, "Could not record the files a queued push changes");
    }
    let local = local_work(hub, repo, target).await;
    remember_local(&repo.top, local.clone());
    let mut others = overlap::open_work(&entries, Utc::now());
    others.extend(local);
    let subject = Work::of_entry(&entry);
    for found in overlap::find(&subject, &others) {
        if told().first_time(&subject, &found) {
            info!(entry = %entry.id, with = %found.with.branch, files = found.files.len(), "Work overlaps");
            for notice in notices(&subject, &found) {
                if hub.notices.send(notice).is_err() {
                    warn!("The loop is gone; an overlap warning was not delivered");
                }
            }
        }
    }
}

/// The work in the local worktrees of `repo` that one agent each works
/// in, on a branch other than `target`.
async fn local_work(hub: &AgentHub, repo: &RepoPaths, target: &str) -> Vec<Work> {
    let mut found = Vec::new();
    for (folder, agent, name) in solo_folders(hub) {
        let in_repo = RepoPaths::of(&folder)
            .await
            .is_ok_and(|theirs| theirs.common == repo.common);
        if !in_repo {
            continue;
        }
        match overlap::worktree_work(&folder, target, GIT_TIMEOUT).await {
            Ok(Some((branch, files))) if !files.is_empty() => found.push(Work {
                entry: None,
                branch,
                agent: Some(agent.to_hex()),
                agent_name: Some(name),
                files,
            }),
            Ok(_) => {}
            Err(why) => warn!(folder = %folder.display(), %why, "Could not read a worktree's changes"),
        }
    }
    found
}

/// Each folder of the registry that exactly one live agent works in,
/// with that agent and its name. A folder agents share has no one owner.
fn solo_folders(hub: &AgentHub) -> Vec<(PathBuf, AgentId, String)> {
    let registry = lock(&hub.registry);
    let mut by_folder: HashMap<PathBuf, Vec<(AgentId, String)>> = HashMap::new();
    for agent in registry.agents() {
        if agent.removed || agent.archived {
            continue;
        }
        if let Some(folder) = &agent.folder {
            by_folder
                .entry(folder.clone())
                .or_default()
                .push((agent.agent_id(), agent.name.clone()));
        }
    }
    by_folder
        .into_iter()
        .filter_map(|(folder, mut agents)| {
            let (agent, name) = agents.pop()?;
            agents.is_empty().then_some((folder, agent, name))
        })
        .collect()
}

/// The notices for `subject` overlapping `found`: one to each owner
/// whose agent is known.
fn notices(subject: &Work, found: &Overlap) -> Vec<Notice> {
    let files = file_list(&found.files);
    let mut out = Vec::new();
    for (mine, theirs) in [(subject, &found.with), (&found.with, subject)] {
        let Some(to) = mine.agent.as_deref().and_then(|hex| AgentId::from_hex(hex).ok()) else {
            continue;
        };
        let other = theirs.owner();
        out.push(Notice {
            to,
            shown: format!(
                "Merge queue: {} changes the same files as {} ({other}): {files}.",
                mine.place(),
                theirs.place()
            ),
            prompt: format!(
                "Heads-up from the merge queue: your work `{}` ({}) changes the same files as \
                 {other}'s `{}` ({}): {files}.\n\nNothing is blocked. Message {other} with \
                 send_to_agent to agree who lands first, and have the other pull main and \
                 resolve before it ships, so the second merge doesn't conflict. Keep it short; \
                 then carry on with your task.",
                mine.branch,
                mine.place(),
                theirs.branch,
                theirs.place(),
            ),
            inbox: None,
        });
    }
    out
}

/// The overlaps of each open entry of `entries` (oldest first), as the
/// QUEUE tab lists them: `q12 aura/x (RUS1): src/a.rs, src/b.rs`.
pub(super) fn entry_overlaps(entries: &[MergeEntry], repo: &str) -> HashMap<EntryId, Vec<String>> {
    let mut others = overlap::open_work(entries, Utc::now());
    let open: Vec<Work> = others.clone();
    others.extend(local_of(Path::new(repo)));
    open.into_iter()
        .filter_map(|work| {
            let lines: Vec<String> = overlap::find(&work, &others)
                .iter()
                .map(|found| {
                    format!(
                        "{} ({}): {}",
                        found.with.place(),
                        found.with.owner(),
                        file_list(&found.files)
                    )
                })
                .collect();
            Some((work.entry?, lines)).filter(|(_, lines)| !lines.is_empty())
        })
        .collect()
}

/// `files`, the first [`SHOWN_FILES`] of them, then how many more.
fn file_list(files: &[String]) -> String {
    let shown = files
        .iter()
        .take(SHOWN_FILES)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    match files.len().saturating_sub(SHOWN_FILES) {
        0 => shown,
        more => format!("{shown} and {more} more"),
    }
}

/// The pairs of work told about their overlap.
#[derive(Default)]
pub(super) struct Told(HashSet<String>);

impl Told {
    /// Whether `subject` and `found` haven't been told about these files
    /// yet; remembers that they now are.
    pub(super) fn first_time(&mut self, subject: &Work, found: &Overlap) -> bool {
        let mut pair = [subject.branch.as_str(), found.with.branch.as_str()];
        pair.sort_unstable();
        let key = format!("{}\n{}\n{}", pair[0], pair[1], found.files.join("\n"));
        self.0.insert(key)
    }
}

fn told() -> std::sync::MutexGuard<'static, Told> {
    static TOLD: OnceLock<Mutex<Told>> = OnceLock::new();
    lock(TOLD.get_or_init(Mutex::default))
}

fn local_cache() -> std::sync::MutexGuard<'static, HashMap<PathBuf, Vec<Work>>> {
    static LOCAL: OnceLock<Mutex<HashMap<PathBuf, Vec<Work>>>> = OnceLock::new();
    lock(LOCAL.get_or_init(Mutex::default))
}

fn remember_local(top: &Path, work: Vec<Work>) {
    local_cache().insert(top.to_path_buf(), work);
}

/// The local worktrees' work last read for the repo whose top is `top`.
fn local_of(top: &Path) -> Vec<Work> {
    local_cache().get(top).cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests;
