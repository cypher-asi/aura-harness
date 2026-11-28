//! Which repos the channels' projects are in, for the merge queues.

use crate::event_loop::subagents::AgentHub;
use aura_exec_merge_queue::RepoPaths;
use std::path::{Path, PathBuf};

/// Whether `repo` has a queue file.
pub(super) async fn has_queue_file(repo: &RepoPaths) -> bool {
    let file = repo
        .queue_dir()
        .join(aura_exec_merge_queue::store::QUEUE_FILE);
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
async fn channel_folders(hub: &AgentHub) -> Vec<(String, PathBuf)> {
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

/// The folder of channel `channel_id`'s project, when it has one.
pub(super) async fn folder_of(hub: &AgentHub, channel_id: &str) -> Option<PathBuf> {
    let folders = channel_folders(hub).await;
    folders
        .into_iter()
        .find(|(id, _)| id == channel_id)
        .map(|(_, folder)| folder)
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
