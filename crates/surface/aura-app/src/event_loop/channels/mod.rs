//! Channels on the event loop: making a channel together with its
//! coordinator, the project its agents work in, the `<channel>` block a
//! coordinator's turns get, the channels list the UI shows, and the UI's
//! channel actions.
//!
//! # Invariants
//!
//! * A channel and its coordinator's agent row land in one batch
//!   ([`ChannelStore::create`]); the coordinator joins the in-memory
//!   registry only after that batch landed, so a failed create leaves the
//!   registry as it was.
//! * A channel with a project has its coordinator at the top of that
//!   project (no folder of its own), so the coordinator and every agent
//!   under it are listed in the Projects tree. A channel without one has
//!   its coordinator in the launch project working in
//!   `<data_dir>/channels/<slug>/`, and its members are left out of the
//!   Projects tree ([`AgentHub::hidden_members`]). A channel naming a
//!   project the registry doesn't list (unknown or removed) is treated as
//!   having none.
//! * Every member is in its coordinator's project. Every member but the
//!   coordinator has a folder of its own, pinned when it joins (or when
//!   the channel's project changes), so moving the coordinator never
//!   moves the work of agents spawned before.
//! * A project change writes the channel record and every moved agent's
//!   row in one batch, under the registry lock, and the registry changes
//!   in memory only once it landed ([`AgentRegistry::place_agents`]).
//!   Joins and project changes read the channel under the registry lock,
//!   so they serialize.
//! * The registry lock is never held across an `await`; the channel
//!   store is read and written off the async runtime, but for the short
//!   synchronous joins and prompt reads documented on their functions.

use super::subagents::{lock, AgentHub};
use super::LoopState;
use crate::agent_definitions::COORDINATOR_DEFINITION;
use crate::agent_registry::{AgentPlacement, AgentRegistry, CoordinatorHome, RegisteredAgent};
use anyhow::{bail, Context};
use aura_core_types::AgentId;
use aura_store_db::agents::derive_agent_id;
use aura_store_db::{
    valid_slug, ChannelRecord, ChannelStore, ChannelUpdate, MemberRecord, MemberRole, NewChannel,
    RegistryDiff, WriteBatch,
};
use aura_ui::events::{nest_members, AgentSummary, ChannelProjectView, ChannelSummary};
use aura_ui::UiCommand;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};

mod chat;
mod create;
mod handlers;
mod join;
mod list;
mod load;
mod prompt;
mod update;

pub(crate) use chat::{
    drop_unanswered, post_replies, route_user_message, send_posts_of, take_digest, ChannelChat,
};
pub(crate) use create::create_channel;
pub(super) use handlers::{
    handle_archive, handle_close, handle_create, handle_open, handle_update,
};
pub(crate) use join::join_or_warn;
#[cfg(test)]
pub(crate) use join::join_parent_channel;
use list::project_view;
#[cfg(test)]
use list::MISSING_PROJECT;
pub(crate) use list::{
    channel_in_project, channel_summaries, member_ids, open_channel_record, send_channels,
    send_channels_with,
};
#[cfg(test)]
use load::load_members;
pub(crate) use load::{load_channels, ChannelSets};
use load::{move_placements, stage_rows};
pub(crate) use prompt::{channel_prompt, is_coordinator};
pub(crate) use update::{archive_channel, update_channel, CoordinatorBusy};

/// Directory under the data directory holding each channel's folder.
const CHANNELS_DIR: &str = "channels";

/// The folder a channel without a project works in.
fn channel_folder(data_dir: &Path, slug: &str) -> PathBuf {
    data_dir.join(CHANNELS_DIR).join(slug)
}

/// Whether `project_id` names a listed (registered, not removed) project
/// of `registry`.
pub(crate) fn is_known_project(registry: &AgentRegistry, project_id: &str) -> bool {
    registry.project(project_id).is_some_and(|p| !p.removed)
}

/// Whether `channel` has a project its agents can work in: one it names
/// that `registry` lists. A channel naming an unknown or removed project
/// is treated as having none: its members are hidden.
fn has_known_project(registry: &AgentRegistry, channel: &ChannelRecord) -> bool {
    channel
        .project_id
        .as_deref()
        .is_some_and(|project_id| is_known_project(registry, project_id))
}

/// Fail unless `project_id` names a listed project of `registry`.
fn check_project(registry: &AgentRegistry, project_id: &str) -> anyhow::Result<()> {
    match registry.project(project_id) {
        Some(project) if !project.removed => Ok(()),
        Some(project) => bail!("project {} was removed", project.name()),
        None => bail!("there is no project {project_id}"),
    }
}

/// `project_id` trimmed, `None` when blank.
fn trimmed_project(project_id: Option<String>) -> Option<String> {
    project_id
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
}

/// Run `read` on a clone of `store` off the async runtime.
async fn blocking<T: Send + 'static>(
    store: &ChannelStore,
    read: impl FnOnce(&ChannelStore) -> Result<T, aura_store_db::ChannelError> + Send + 'static,
) -> anyhow::Result<T> {
    let store = store.clone();
    let result = tokio::task::spawn_blocking(move || read(&store))
        .await
        .context("channel store task failed")?;
    Ok(result?)
}

/// The channel store, or why channels are unavailable.
fn store_of(hub: &AgentHub) -> anyhow::Result<&ChannelStore> {
    hub.channels
        .as_ref()
        .context("channels are not available without the agent database")
}

/// The channel `channel_id`, unless it is missing or archived; read
/// synchronously.
fn open_record(store: &ChannelStore, channel_id: &str) -> anyhow::Result<ChannelRecord> {
    let channel = store
        .get(channel_id)
        .with_context(|| format!("reading channel {channel_id}"))?;
    match channel {
        Some(channel) if !channel.archived => Ok(channel),
        Some(channel) => bail!("#{} is archived", channel.slug),
        None => bail!("there is no channel {channel_id}"),
    }
}

#[cfg(test)]
mod tests;
