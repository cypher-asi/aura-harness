//! Member worktrees following their agents into and out of the archive
//! ([`crate::isolation::member`]).

use crate::event_loop::subagents::{lock, AgentHub};
use crate::isolation::member::{
    is_member_worktree, member_branch, provision_member_worktree, remove_member_worktree, Removal,
};
use aura_core_types::AgentId;
use std::path::PathBuf;
use tracing::{info, warn};

/// The member worktree each of `agents` works in, if any: its own
/// folder, under the data dir's worktrees.
fn worktrees_of(hub: &AgentHub, agents: &[AgentId]) -> Vec<(AgentId, PathBuf)> {
    let registry = lock(&hub.registry);
    let data_dir = registry.data_dir().to_path_buf();
    agents
        .iter()
        .filter_map(|&agent| {
            let folder = registry.find(agent)?.folder.clone()?;
            is_member_worktree(&data_dir, &folder)
                .then(|| (agent, worktree_root(&data_dir, &folder)))
        })
        .collect()
}

/// The worktree `folder` is in: its first path part under `worktrees/`.
fn worktree_root(data_dir: &std::path::Path, folder: &std::path::Path) -> PathBuf {
    let base = data_dir.join("worktrees");
    folder
        .strip_prefix(&base)
        .ok()
        .and_then(|rest| rest.components().next())
        .map_or_else(|| folder.to_path_buf(), |first| base.join(first))
}

/// `agents` were archived: remove their worktrees that hold no
/// uncommitted work (logged when one is kept). Their branches stay.
pub(super) async fn release(hub: &AgentHub, agents: &[AgentId]) {
    let found = worktrees_of(hub, agents);
    if found.is_empty() {
        return;
    }
    // A join error means the blocking pool is gone: the app is closing.
    let _ = tokio::task::spawn_blocking(move || {
        for (agent, worktree) in found {
            match remove_member_worktree(&worktree) {
                Ok(Removal::Kept) => info!(
                    %agent,
                    worktree = %worktree.display(),
                    "Kept an archived member's worktree: it holds uncommitted work"
                ),
                Ok(_) => {}
                Err(err) => {
                    warn!(%agent, error = %err, "Could not remove an archived member's worktree");
                }
            }
        }
    })
    .await;
}

/// `agents` were restored: add back the worktrees archiving removed, on
/// their branches, from their project's folder.
pub(super) async fn restore(hub: &AgentHub, agents: &[AgentId]) {
    let wanted: Vec<(AgentId, PathBuf, PathBuf, String)> = {
        let found = worktrees_of(hub, agents);
        let registry = lock(&hub.registry);
        found
            .into_iter()
            .filter(|(_, worktree)| !worktree.exists())
            .filter_map(|(agent, worktree)| {
                let shared = registry.project_of(agent)?.folder.clone();
                let definition = registry.find(agent)?.definition.clone();
                let branch = member_branch(definition.as_deref(), &agent.to_hex());
                Some((agent, shared, worktree, branch))
            })
            .collect()
    };
    if wanted.is_empty() {
        return;
    }
    // A join error means the blocking pool is gone: the app is closing.
    let _ = tokio::task::spawn_blocking(move || {
        for (agent, shared, worktree, branch) in wanted {
            if let Err(err) = provision_member_worktree(&shared, &worktree, &branch) {
                warn!(%agent, error = %err, "Could not add back a restored member's worktree");
            }
        }
    })
    .await;
}
