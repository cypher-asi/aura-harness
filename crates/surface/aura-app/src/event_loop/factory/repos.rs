//! Where the hub's merge queues are: the channels' repos, their
//! coordinators, and the forge each repo's PRs live on.

use crate::event_loop::subagents::AgentHub;
use aura_core_types::AgentId;
use aura_exec_merge_queue::forge::{Forge, GhForge};
use aura_exec_merge_queue::RepoPaths;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::info;

/// The forge the PRs of the repo at `top` live on: GitHub through `gh`
/// (its own sign-in, or `GH_TOKEN`/`GITHUB_TOKEN`) when `origin` is on
/// github.com; `None` lands entries by push.
pub(super) async fn forge_of(top: &Path) -> Option<Arc<dyn Forge>> {
    let slug = aura_exec_merge_queue::forge::origin_slug(top).await?;
    info!(repo = %slug, "The merge queue lands this repo through its pull requests");
    let forge = GhForge::new(slug).with_token_source(Arc::new(github_token));
    Some(Arc::new(forge))
}

/// The token the queue's `gh` gets, in the REPO tab's order: none while
/// `GH_TOKEN` or `GITHUB_TOKEN` is set (`gh` reads those itself), else
/// the token stored in the Integrations panel, else none (`gh`'s own
/// sign-in). Read on each call, so a token stored later is used.
fn github_token() -> Option<String> {
    let set = |name| std::env::var(name).is_ok_and(|value: String| !value.trim().is_empty());
    if set("GH_TOKEN") || set("GITHUB_TOKEN") {
        return None;
    }
    crate::integrations::get().and_then(|integrations| integrations.github_token())
}

/// Whether `repo` has a queue file.
pub(super) async fn has_queue_file(repo: &RepoPaths) -> bool {
    let file = repo.queue_dir().join(aura_exec_merge_queue::QUEUE_FILE);
    tokio::fs::try_exists(&file).await.unwrap_or(false)
}

/// The folders of the channels' projects.
pub(super) async fn project_folders(hub: &AgentHub) -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = channel_folders(hub)
        .await
        .into_iter()
        .map(|(_, folder)| folder)
        .collect();
    folders.sort();
    folders.dedup();
    folders
}

/// Each channel with a project, and its project's folder.
pub(super) async fn channel_folders(hub: &AgentHub) -> Vec<(String, PathBuf)> {
    let summaries = crate::event_loop::channels::channel_summaries(hub)
        .await
        .unwrap_or_default();
    summaries
        .into_iter()
        .filter_map(|channel| {
            let folder = channel.project?.folder;
            (!folder.is_empty()).then(|| (channel.id, PathBuf::from(folder)))
        })
        .collect()
}

/// The channels whose project is in the repo whose common git dir is
/// `common`.
pub(super) async fn channels_on(hub: &AgentHub, common: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for (channel_id, folder) in channel_folders(hub).await {
        if RepoPaths::of(&folder)
            .await
            .is_ok_and(|repo| repo.common == common)
        {
            found.push(channel_id);
        }
    }
    found
}

/// The coordinators of the channels whose project is in the repo whose
/// common git dir is `common`.
pub(super) async fn coordinators_on(hub: &AgentHub, common: &Path) -> Vec<AgentId> {
    let summaries = crate::event_loop::channels::channel_summaries(hub)
        .await
        .unwrap_or_default();
    let mut found = Vec::new();
    for channel in summaries {
        let Some(folder) = channel.project.map(|project| project.folder) else {
            continue;
        };
        let on_repo = RepoPaths::of(Path::new(&folder))
            .await
            .is_ok_and(|repo| repo.common == common);
        if let (true, Ok(id)) = (on_repo, AgentId::from_hex(&channel.coordinator_id)) {
            found.push(id);
        }
    }
    found
}
