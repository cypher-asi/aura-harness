//! Which agent a session opens on: the chat the UI was in when it last
//! closed, so the first chat the user sees is the one they left and it
//! is never loaded twice.

use crate::agent_registry::{AgentRegistry, RegisteredAgent};
use aura_core_types::AgentId;
use aura_store_db::{ChannelStore, RocksStore};
use std::path::PathBuf;
use tracing::{debug, warn};

/// The chat a session opens on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenOn {
    /// A channel, by id: its coordinator's chat
    Channel(String),
    /// An agent, by hex id
    Agent(String),
}

impl OpenOn {
    /// What a UI's saved selection opens on: the channel it had open when
    /// channels were in front, else the agent it was last on, if any.
    #[must_use]
    pub fn from_last(
        channels_in_front: bool,
        channel: Option<&str>,
        agent: Option<&str>,
    ) -> Option<Self> {
        match (channels_in_front, channel, agent) {
            (true, Some(channel), _) => Some(Self::Channel(channel.to_string())),
            (_, _, Some(agent)) => Some(Self::Agent(agent.to_string())),
            _ => None,
        }
    }
}

/// The data directory `options` name: `dir`, else `AURA_DATA_DIR`, else
/// `./aura_data`. A UI reads what it saved there before the session
/// starts.
#[must_use]
pub fn data_dir(options: &super::SessionOptions) -> PathBuf {
    options
        .dir
        .clone()
        .or_else(|| std::env::var("AURA_DATA_DIR").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("./aura_data"))
}

/// The agent to open on: `open_on`'s, made the registry's active one,
/// when it is a listed agent (or a listed channel's coordinator); else
/// the registry's active agent.
pub(super) fn agent_to_open(
    registry: &mut AgentRegistry,
    store: &RocksStore,
    open_on: Option<&OpenOn>,
) -> RegisteredAgent {
    if let Some(agent) = open_on.and_then(|open_on| wanted(registry, store, open_on)) {
        if registry.active().agent_id() != agent {
            if let Err(error) = registry.set_active(agent) {
                warn!(%agent, %error, "Could not open on the chat the app was last in");
            }
        }
    }
    registry.active().clone()
}

/// The agent `open_on` names, when it is listed and not archived.
fn wanted(registry: &AgentRegistry, store: &RocksStore, open_on: &OpenOn) -> Option<AgentId> {
    let agent = match open_on {
        OpenOn::Agent(hex) => AgentId::from_hex(hex).ok()?,
        OpenOn::Channel(id) => match ChannelStore::for_store(store).get(id) {
            Ok(Some(channel)) if !channel.archived => channel.coordinator,
            Ok(_) => return None,
            Err(error) => {
                debug!(channel = %id, %error, "Could not read the channel the app was last in");
                return None;
            }
        },
    };
    registry
        .find(agent)
        .filter(|found| !found.archived)
        .map(RegisteredAgent::agent_id)
}

#[cfg(test)]
mod tests;
