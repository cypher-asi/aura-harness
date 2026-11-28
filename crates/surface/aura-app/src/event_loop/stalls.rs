//! The stall sweep: once a minute, find agents that owe answers but have
//! nothing running, moving or queued, and are not waiting on the user,
//! so none sits idle forever while another waits on its report.
//!
//! An agent seen that way on [`RETRY_AFTER`] sweeps in a row is asked
//! once to carry on, with what it owes. Seen that way as many sweeps
//! again, what it owes is given up, telling each sender, and its peers
//! see it as stalled. Queued work left with nothing running is started.
//!
//! # Invariants
//!
//! * An agent the user stopped to carry on (PR #60's parked reports) is
//!   left alone until its next work starts ([`super::inbox::Inbox::park`]).
//! * A sweep never starts work for an agent that is working or moving.

use super::delivery::{deliver, start_next};
use super::steering::SteerSource;
use super::subagents::lock;
use super::LoopState;
use aura_core_types::AgentId;
use std::collections::HashSet;
use std::time::Duration;
use tracing::{info, warn};

/// How often the sweep runs.
pub(super) const SWEEP_EVERY: Duration = Duration::from_secs(60);

/// Sweeps in a row an agent is seen stalled before it is retried, and
/// again before it is given up on.
const RETRY_AFTER: u8 = 2;

/// Why the reports of an agent the sweep gives up on are not coming.
pub(super) const WENT_IDLE: &str =
    "it went idle without finishing, and asking it to carry on did not help";

/// One sweep. Also brings the placed members' reaches up to date.
pub(super) async fn sweep(state: &mut LoopState<'_>) {
    super::cloud::refresh_reach(state).await;
    let owing = state.hub.inbox.owing();
    let ids: HashSet<AgentId> = owing.iter().map(|(agent, _)| *agent).collect();
    state.hub.standing.keep_strikes(&ids);
    for (agent, asks) in owing {
        if !is_idle(state, agent) {
            state.hub.standing.unstrike(agent);
            continue;
        }
        if state.queued.len(agent) > 0 {
            info!(%agent, "Stall sweep: queued work with nothing running; starting it");
            state.hub.standing.unstrike(agent);
            start_next(state, agent).await;
            continue;
        }
        match state.hub.standing.strike(agent) {
            (count, false) if count >= RETRY_AFTER => retry(state, agent, &asks).await,
            (count, true) if count >= RETRY_AFTER => give_up(state, agent),
            _ => {}
        }
    }
}

/// Whether `agent` has nothing running, moving or starting, and is an
/// agent the sweep may act for.
fn is_idle(state: &LoopState<'_>, agent: AgentId) -> bool {
    let live = lock(&state.hub.registry)
        .find(agent)
        .is_some_and(|found| !found.archived && !found.removed);
    live && !state.hub.is_working(agent)
        && !state.cloud.is_running(agent)
        && !state.cloud.is_moving(agent)
        && !state.hub.status.is_starting(agent)
        && !lock(&state.hub.cloud_starts).contains(&agent)
}

/// Ask `agent` once to carry on with what it owes, `asks` (subject and
/// marker each), so its report answers them.
async fn retry(state: &mut LoopState<'_>, agent: AgentId, asks: &[(String, String)]) {
    warn!(%agent, owed = asks.len(), "Stall sweep: idle while owing reports; asking it to carry on");
    state.hub.standing.retried(agent);
    deliver(state, agent, SteerSource::Harness, carry_on(asks), false).await;
}

/// The prompt asking an agent to carry on with `asks`.
pub(super) fn carry_on(asks: &[(String, String)]) -> String {
    let mut text = String::from(
        "You went idle while you still owe these reports, so carry on where you left off:\n",
    );
    for (subject, marker) in asks {
        text.push_str(&format!("- {marker}\"{subject}\"\n"));
    }
    text.push_str(
        "\nFinish that work, then end your turn with your report: it goes back on its own. If \
         you are blocked, end your turn saying what blocks you.",
    );
    text
}

/// Give up what `agent` owes: retrying it did not help. Its row and its
/// peers show it stalled until its next turn starts.
fn give_up(state: &LoopState<'_>, agent: AgentId) {
    warn!(%agent, "Stall sweep: still idle after a retry; its reports are given up");
    super::inbox::give_up_all(&state.hub, agent, WENT_IDLE);
    state.hub.status.fail(agent, WENT_IDLE);
    state.hub.standing.stall(agent);
}
