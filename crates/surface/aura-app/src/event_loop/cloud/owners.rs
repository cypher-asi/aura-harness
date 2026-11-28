//! Only the user who placed a cloud agent manages it.

use super::*;

/// Why the user signed in here may not manage cloud agent `agent`: only
/// the user who placed it can move, pull, sleep, wake, archive or talk
/// to it.
pub(super) fn foreign_owner(
    state: &LoopState<'_>,
    hub: &CloudHub,
    agent: AgentId,
) -> Option<String> {
    let owner = hub.owner_of(agent)?;
    refusal_for(&owner, hub.owner_name_of(agent), &state.auth.owner())
}

/// The notice when `current` is not `owner` (named `owner_name`).
pub(super) fn refusal_for(
    owner: &aura_core_auth::Owner,
    owner_name: Option<String>,
    current: &aura_core_auth::Owner,
) -> Option<String> {
    if owner == current {
        return None;
    }
    let name = owner_name.unwrap_or_else(|| match owner {
        aura_core_auth::Owner::Local => "this machine's local profile".to_string(),
        aura_core_auth::Owner::Zos { .. } => "another user".to_string(),
    });
    Some(format!("Placed by {name}; sign in as {name} to manage it."))
}

/// Why `agents` may not be archived by the user signed in here: one of
/// them is another user's cloud agent.
pub(in crate::event_loop) fn archive_refusal(
    state: &LoopState<'_>,
    agents: &[AgentId],
) -> Option<String> {
    let hub = state.cloud.hub()?;
    agents
        .iter()
        .find_map(|agent| foreign_owner(state, &hub, *agent))
}
