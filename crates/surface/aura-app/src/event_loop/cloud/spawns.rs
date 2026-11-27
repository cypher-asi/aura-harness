//! Agents cloud agents spawned on their machines, registered here as a
//! spawn here registers its own: under the agent that spawned it, in its
//! project, and a member of its channel. Their machine's node stays the
//! only writer of their records ([`CloudState::node_writes`]).

use super::*;
use crate::agent_registry::{RegisteredAgent, Spawned};
use crate::event_loop::channels::join_or_warn;
use crate::event_loop::titles::fallback_subtitle;
use aura_agent_kernel::{spawned_as, SpawnedAs};
use aura_store_db::ReadStore;

/// The role a `task` gives the agent it spawns, before its kind.
const TASK_ROLE_PREFIX: &str = "subagent:";

/// How many entries at the head of a spawned agent's record are read for
/// the prompt it was spawned with.
const FIRST_PROMPT_WITHIN: usize = 16;

/// A spawned agent not registered yet.
struct Unlisted {
    agent: AgentId,
    spawned: SpawnedAs,
    /// What it was spawned to do, from its first prompt.
    subtitle: Option<String>,
}

/// Register the agents of `spawns`, `(placed, spawned)` pairs, that are
/// not registered yet, each once its record names the agent that spawned
/// it and that agent is registered
/// ([`crate::agent_registry::AgentRegistry::adopt_spawn`]), then add each
/// to its spawner's channel. An agent spawned by one registered here
/// follows it. Reads records and writes the registry and the channel
/// store: callers on the runtime run it on a blocking thread. Returns
/// whether any agent was registered.
pub(super) fn adopt_machine_spawns(hub: &AgentHub, spawns: &[(AgentId, AgentId)]) -> bool {
    let unknown: Vec<AgentId> = {
        let registry = lock(&hub.registry);
        spawns
            .iter()
            .map(|&(_, spawned)| spawned)
            .filter(|&agent| registry.find(agent).is_none())
            .collect()
    };
    let store = hub.factory.store.as_ref();
    let mut unlisted: Vec<Unlisted> = unknown
        .into_iter()
        .filter_map(|agent| {
            Some(Unlisted {
                agent,
                spawned: spawned_as(store, agent)?,
                subtitle: first_prompt(store, agent).map(|prompt| fallback_subtitle(&prompt)),
            })
        })
        .collect();
    let mut adopted = false;
    loop {
        let registered = register_ready(hub, &mut unlisted);
        if registered.is_empty() {
            return adopted;
        }
        adopted = true;
        for agent in &registered {
            join_or_warn(hub, agent);
        }
    }
}

/// Register the agents of `unlisted` whose spawner is registered, taking
/// them out of it. Returns them as registered.
fn register_ready(hub: &AgentHub, unlisted: &mut Vec<Unlisted>) -> Vec<RegisteredAgent> {
    let mut registry = lock(&hub.registry);
    let mut registered = Vec::new();
    unlisted.retain(|waiting| {
        let Some(parent) = waiting
            .spawned
            .parent
            .and_then(|parent| registry.find(parent))
            .map(|parent| parent.zns_id.clone())
        else {
            return true;
        };
        let spawned = Spawned {
            agent_id: waiting.agent,
            parent: &parent,
            kind: waiting.spawned.role.strip_prefix(TASK_ROLE_PREFIX),
            name: &waiting.spawned.name,
            subtitle: waiting.subtitle.as_deref(),
        };
        match registry.adopt_spawn(&spawned) {
            Ok(agent) => {
                info!(
                    agent = %waiting.agent.to_hex(),
                    name = %agent.name,
                    %parent,
                    "Listed an agent a cloud agent spawned on its machine"
                );
                registered.push(agent);
            }
            Err(error) => warn!(
                agent = %waiting.agent.to_hex(),
                %error,
                "Could not list an agent a cloud agent spawned on its machine"
            ),
        }
        false
    });
    registered
}

/// The first prompt in `agent`'s record: what it was spawned to do.
fn first_prompt(store: &dyn ReadStore, agent: AgentId) -> Option<String> {
    store
        .scan_record(agent, 1, FIRST_PROMPT_WITHIN)
        .ok()?
        .into_iter()
        .find_map(|entry| entry.tx.prompt_payload())
        .map(|prompt| prompt.text)
}
