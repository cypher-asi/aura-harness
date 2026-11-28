//! What a cloud turn notes as its events come: on its agent's rows, that
//! it waits for the one under way on its machine or that Aura reconnects
//! to it; and, for its chat's boot console, whether the machine wakes.

use super::*;

/// What a cloud turn of `agent` notes for its rows and its peers as it
/// goes: that it waits for the one already under way on its machine (on
/// while it does, off once anything else of the turn comes, and off when
/// the turn ends), and that it implements. Its machine doesn't hold it to
/// plan mode, so a turn asked to plan can start editing, committing or
/// pushing; it is out of plan mode from that call on.
pub(in crate::event_loop) struct WaitingMark {
    pub(in crate::event_loop) hub: Arc<AgentHub>,
    pub(in crate::event_loop) agent: AgentId,
}

impl WaitingMark {
    /// Note it on or off, `waiting`; the rows show it at once.
    pub(super) async fn set(&self, waiting: bool) {
        self.note(waiting, false).await;
    }

    /// Note whether the turn waits and whether it is being reconnected
    /// to; the rows show a change at once.
    async fn note(&self, waiting: bool, reconnecting: bool) {
        let status = &self.hub.status;
        let waits = status.set_waiting(self.agent, waiting);
        let reconnects = status.set_reconnecting(self.agent, reconnecting);
        if waits || reconnects {
            self.hub.send_agent_list().await;
        }
    }

    /// Note what `event` says of the turn: a status of waiting or of
    /// reconnecting holds until anything else of the turn comes.
    pub(super) async fn see(&self, event: &RemoteEvent) {
        self.note(says_waiting(event), says_reconnecting(event))
            .await;
        if let RemoteEvent::ToolStart { name, .. } = event {
            if crate::event_loop::modes::implements(name) {
                // Its task builds now, whether or not a plan was written.
                let built = self.hub.factory_flows.build_unplanned(self.agent);
                let left =
                    crate::event_loop::modes::leave_plan_mode_of(&self.hub, self.agent).await;
                if built || left {
                    info!(agent = %self.agent, tool = %name, "A cloud turn asked to plan implements; it builds, out of plan mode");
                }
            }
        }
    }
}

/// Whether `event` says the app reconnects to its running turn.
fn says_reconnecting(event: &RemoteEvent) -> bool {
    matches!(event, RemoteEvent::Status(status) if status == crate::cloud::hub::RECONNECTING)
}

/// Whether `event` says its turn waits for the one under way on its
/// machine.
fn says_waiting(event: &RemoteEvent) -> bool {
    matches!(event, RemoteEvent::Status(status) if status == crate::cloud::hub::WAITING)
}

/// A boot console's title when the machine wakes.
pub(super) const WAKING: &str = "Waking cloud machine";

/// Whether `event`, a machine's first boot step, is a wake.
pub(super) fn wakes(event: &BootEvent) -> bool {
    use aura_engine_cloud::BootStage;
    match event {
        BootEvent::Plan(stages) => stages.contains(&BootStage::Wake),
        BootEvent::Stage { stage, .. }
        | BootEvent::Log { stage, .. }
        | BootEvent::Progress { stage, .. } => *stage == BootStage::Wake,
        _ => false,
    }
}
