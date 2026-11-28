//! A cloud agent while the hub is down.
//!
//! # Invariant
//!
//! An agent the saved placements put on a cloud machine never runs on
//! this computer because the hub isn't connected: without the hub it only
//! looks local ([`CloudState::is_cloud`] needs a hub). Its work first
//! connects the hub. When that fails, the work is turned away: the agent
//! fails in red, its chat says why, and what it owed is given up with
//! the reason.

use super::submit::refused;
use super::*;

impl CloudState {
    /// Whether the placements saved here put `agent` on a cloud machine.
    /// Only asked while the hub is down; read off the runtime.
    async fn saved_on_cloud(&self, agent: AgentId) -> bool {
        let Some(home) = self.saved.clone() else {
            return false;
        };
        let saved =
            tokio::task::spawn_blocking(move || aura_engine_cloud::hub::saved_cloud_agents(&home))
                .await;
        saved.is_ok_and(|agents| agents.contains(&agent))
    }
}

/// Read the cloud agents the hub saved into what the UI marks, so they
/// show their cloud from the start, hub or no hub. Off the runtime.
pub(super) async fn load_saved(state: &LoopState<'_>) {
    let Some(home) = state.cloud.saved.clone() else {
        return;
    };
    let read = move || aura_engine_cloud::hub::saved_placements(&home);
    if let Ok(saved) = tokio::task::spawn_blocking(read).await {
        info!(agents = saved.len(), "Read the cloud agents the hub saved");
        *state.cloud.shared.saved.lock() = saved;
    }
}

/// Before work for `agent` is placed: when the hub is down but the saved
/// placements put `agent` on a cloud machine, connect the hub first.
/// Returns whether the work may go on (to the cloud, or here when the
/// hub, now up, says the agent runs here); when it may not, the agent
/// failed and the work is dropped.
pub(super) async fn connect_for(state: &mut LoopState<'_>, agent: AgentId) -> bool {
    if state.cloud.hub().is_some() || !state.cloud.saved_on_cloud(agent).await {
        return true;
    }
    info!(%agent, "A cloud agent's work came while the cloud is down; connecting first");
    let Err(why) = connect(state, None).await else {
        return true;
    };
    let note = offline_note(&why);
    state.hub.status.fail(agent, &note);
    refused(state, agent, note).await;
    false
}

/// What a cloud agent's chat, and whoever is owed its answer, are told
/// when its work came while the cloud couldn't be connected, `why`.
fn offline_note(why: &str) -> String {
    let why = why.trim().trim_end_matches('.');
    format!(
        "Not run: this agent runs on a cloud machine, and the cloud can't be reached \
         ({why}). It did not run on this computer instead; send it again once the cloud is up."
    )
}

#[cfg(test)]
impl CloudState {
    /// Read saved placements from `home`, as the app reads its own.
    pub(in crate::event_loop) fn read_saved_from(&mut self, home: &Path) {
        self.saved = Some(home.to_path_buf());
    }

    /// Note a cloud turn of `agent` under way, as submitting one does.
    pub(in crate::event_loop) fn start_turn(&self, agent: AgentId) {
        self.shared
            .running
            .lock()
            .insert(agent, CancellationToken::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_note_says_it_did_not_run_here() {
        assert_eq!(
            offline_note(" no boxd key. "),
            "Not run: this agent runs on a cloud machine, and the cloud can't be reached \
             (no boxd key). It did not run on this computer instead; send it again once the \
             cloud is up."
        );
    }
}
