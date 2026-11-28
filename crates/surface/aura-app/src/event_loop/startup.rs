//! What the loop does at startup once it takes the UI's events: the side
//! panels of the chat on screen, the cloud hub, resuming what the last
//! run cut short, then the rest, one step at a time, each only while no
//! UI event waits. The chat the user is in is usable from the first
//! frame; nothing here holds up typing, switching chats or opening a
//! channel.
//!
//! ## Invariants
//!
//! - Before the loop takes any UI event, the saved inbox is restored
//!   ([`super::inbox::restore_saved`]) and the cloud agents the hub
//!   saved are marked: restored queues and owed reports keep their place
//!   ahead of new work, and work for a cloud agent never starts here.
//! - The steps run in [`Step`]'s order, each once. [`Step::Resume`]
//!   comes after [`Step::Cloud`], as resuming needs the hub up, and runs
//!   [`super::inbox::resume_saved`] exactly as a blocking startup did:
//!   cut-short turns and owed reports resume, busy agents are skipped.
//! - The last step tells the UI startup is done
//!   ([`UiCommand::StartupDone`]).
//!
//! A step runs to its end once started (the cloud connect included), so
//! a UI event that comes in meanwhile waits for that step alone.

use super::inbox::{self, Resume};
use super::{agents, cloud, handlers, hud, modes, plan_mode, policies, LoopState};
use aura_core_types::AgentId;
use aura_ui::UiCommand;
use std::collections::VecDeque;
use tracing::debug;

/// One step of startup, in the order they run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Step {
    /// The side panels of the agent on screen
    Panels,
    /// The cloud hub, and the cloud agents it runs
    Cloud,
    /// What the last run cut short: turns, owed reports, queues
    Resume,
    /// The run trace's history and the model picker
    Extras,
    /// Tell the UI startup is done
    Done,
}

/// The steps still to run.
pub(super) struct Startup {
    steps: VecDeque<Step>,
    /// What [`Step::Resume`] resumes, taken by it
    restored: Option<Resume>,
}

impl Startup {
    /// Every step, with `restored` back from the saved inbox.
    pub(super) fn new(restored: Resume) -> Self {
        Self {
            steps: VecDeque::from([
                Step::Panels,
                Step::Cloud,
                Step::Resume,
                Step::Extras,
                Step::Done,
            ]),
            restored: Some(restored),
        }
    }

    /// Whether the next step may run now: one is left and no UI event
    /// waits, as the user's input always goes first.
    pub(super) fn may_run(&self, state: &LoopState<'_>) -> bool {
        !self.steps.is_empty() && state.events.is_empty()
    }

    /// The step that runs next.
    #[cfg(test)]
    pub(super) fn next_step(&self) -> Option<Step> {
        self.steps.front().copied()
    }

    /// Run the next step.
    pub(super) async fn run_next(&mut self, state: &mut LoopState<'_>) {
        let Some(step) = self.steps.pop_front() else {
            return;
        };
        debug!(?step, "Startup step");
        // The agent on screen now: the user may have switched already.
        let active = state.active;
        match step {
            Step::Panels => send_panels(state, active).await,
            Step::Cloud => cloud::start(state).await,
            Step::Resume => {
                if let Some(restored) = self.restored.take() {
                    inbox::resume_saved(state, restored).await;
                }
            }
            Step::Extras => {
                state.hub.traces.send_history(active).await;
                handlers::announce_models(state).await;
            }
            Step::Done => {
                // A closed UI takes nothing more; the loop ends with it.
                let _ = state.commands.send(UiCommand::StartupDone).await;
            }
        }
    }
}

/// What the side panels show of `agent`: its sandbox, session, mode,
/// policies, instructions and plan.
async fn send_panels(state: &mut LoopState<'_>, agent: AgentId) {
    agents::send_sandbox_root(state).await;
    hud::send_session_of(state, agent).await;
    modes::send_mode(state, agent).await;
    policies::send_policies_of(state, agent).await;
    modes::warn_if_unconfined(state, agent).await;
    hud::send_instructions_of(state, agent).await;
    plan_mode::send_plan_of(state, agent).await;
}
