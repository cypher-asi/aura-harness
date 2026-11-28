//! Keeping an agent at its work through trouble it can get past: a model
//! that refused its sign-in, a model out of reach for a while, a cloud
//! machine that failed to start.
//!
//! Such an agent is held ([`Recovery`]) instead of failed: its owed
//! reports stay owed (its inbox is parked, so the stall sweep leaves it
//! be), its queue is kept and waits, and its chat and rows say why.
//! [`tick`], once a minute and whenever a sign-in works again, lets go
//! of the holds that are due:
//!
//! - a sign-in pause, once any model call works again or the user signs
//!   in, and its cooldown is over: the agent is asked to carry on;
//! - a turn that failed on transient trouble while it owed a report,
//!   after 1, 2 then 4 minutes: asked to carry on, up to
//!   [`state::MAX_TURN_RETRIES`] times;
//! - a cloud start, after 1, 2 then 4 minutes for its machine's failure,
//!   or every 10 minutes, up to 6 times, for its project's (a fix may
//!   land meanwhile): started again.
//!
//! Past a cap the agent fails as it did before. Trouble no pause or retry
//! fixes ([`Trouble`] is `None`) fails at once. Work the user sends a
//! held agent lets it go at once ([`release`]).

mod state;
mod trouble;

pub(crate) use state::Recovery;
pub(crate) use trouble::Trouble;

use super::agent_events::AgentUi;
use super::delivery::deliver;
use super::steering::SteerSource;
use super::LoopState;
use aura_core_types::AgentId;
use aura_ui::UiCommand;
use state::Hold;
use tracing::{info, warn};

/// How a turn went, for recovery: the trouble it ended on, and whether a
/// model call worked.
#[derive(Debug, Clone, Default)]
pub(crate) struct TurnHealth {
    /// What went wrong that a pause or a retry may get past
    pub trouble: Option<Trouble>,
    /// The turn's model calls worked: it ended without a model error
    pub model_worked: bool,
    /// The model error the turn ended on, if any
    pub llm_error: Option<String>,
}

fn now() -> u64 {
    aura_core_types::time::now_ms()
}

/// `agent`'s turn ended on `trouble`: hold it, rather than fail it, when
/// a pause or a retry may get it past. Returns whether it is held.
pub(crate) async fn hold_turn(state: &LoopState<'_>, agent: AgentId, trouble: &Trouble) -> bool {
    let (hub, now) = (&state.hub, now());
    let note = match trouble {
        Trouble::SignIn(why) => {
            warn!(%agent, %why, "The model refused the sign-in; waiting for one that works");
            hub.recovery.pause_for_sign_in(agent, why, now);
            sign_in_note(why)
        }
        // A turn owing nothing has nobody waiting on it: it fails as before.
        Trouble::Transient(_) if hub.inbox.owed_by(agent) == 0 => return false,
        Trouble::Transient(why) => match hub.recovery.hold_turn(agent, why, now) {
            Some((attempt, wait)) => {
                warn!(%agent, %why, attempt, "A turn failed on transient trouble; retrying");
                let note = retry_note(why, attempt, wait);
                hub.status.fail(agent, &note);
                note
            }
            None => return false,
        },
    };
    hub.inbox.park(agent);
    AgentUi::new(state.commands.clone(), agent)
        .send(UiCommand::ShowWarning(note))
        .await;
    super::agents::send_agent_list(state).await;
    true
}

/// `agent`'s cloud start failed because of `why`, its project's doing
/// when `blocked`: hold it for a retry, its work kept. Returns whether it
/// is held, with what its chat and row say; past the cap it is not, and
/// fails as before.
pub(crate) fn hold_start(
    state: &LoopState<'_>,
    agent: AgentId,
    why: &str,
    blocked: bool,
) -> Option<String> {
    let (attempt, wait) = state.hub.recovery.hold_start(agent, why, blocked, now())?;
    warn!(%agent, %why, attempt, blocked, "A cloud start failed; retrying");
    state.hub.inbox.park(agent);
    let cap = if blocked {
        state::MAX_BLOCKED_START_RETRIES
    } else {
        state::MAX_START_RETRIES
    };
    Some(format!(
        "Retrying the start in {} (try {attempt} of {cap}); its work waits.",
        minutes(wait)
    ))
}

/// A model call of `agent` worked: every agent waiting for a sign-in may
/// go on, and does once its cooldown is over.
pub(crate) async fn model_worked(state: &mut LoopState<'_>, agent: AgentId) {
    state.hub.recovery.worked(agent);
    tick(state).await;
}

/// `agent`'s cloud start worked.
pub(crate) fn started(state: &LoopState<'_>, agent: AgentId) {
    state.hub.recovery.started(agent);
}

/// The user signed in: every agent waiting for a sign-in may go on.
pub(crate) async fn signed_in(state: &mut LoopState<'_>) {
    state.hub.recovery.signed_in();
    tick(state).await;
}

/// Let go of the holds due now.
pub(crate) async fn tick(state: &mut LoopState<'_>) {
    tick_at(state, now()).await;
}

/// Let go of the holds due at `now`: ask each held turn to carry on, and
/// start each held cloud start again.
pub(crate) async fn tick_at(state: &mut LoopState<'_>, now: u64) {
    for (agent, hold) in state.hub.recovery.take_due(now) {
        let live = super::subagents::lock(&state.hub.registry)
            .find(agent)
            .is_some_and(|found| !found.archived && !found.removed);
        state.hub.inbox.unpark(agent);
        if !live {
            continue;
        }
        match hold {
            Hold::SignIn { .. } => resume(state, agent, SIGNED_IN).await,
            Hold::Turn { why, .. } => resume(state, agent, &after_error(&why)).await,
            Hold::Start { .. } => {
                info!(%agent, "Retrying a cloud start");
                super::cloud::retry_start(state, agent).await;
            }
        }
    }
}

/// Let `agent` go at once, the user having sent it work: its inbox and
/// row as if it had not been held.
pub(crate) fn release(state: &LoopState<'_>, agent: AgentId) {
    if let Some(hold) = state.hub.recovery.release(agent) {
        info!(%agent, ?hold, "The user sent a held agent work; letting it go");
        state.hub.inbox.unpark(agent);
        if !matches!(hold, Hold::SignIn { .. }) {
            state.hub.status.clear(agent);
        }
    }
}

/// Ask `agent` to carry on, `opening` saying why, naming what it owes.
async fn resume(state: &mut LoopState<'_>, agent: AgentId, opening: &str) {
    info!(%agent, "Asking a held agent to carry on");
    state.hub.status.clear(agent);
    let asks = state
        .hub
        .inbox
        .owing()
        .into_iter()
        .find(|(owing, _)| *owing == agent)
        .map(|(_, asks)| asks)
        .unwrap_or_default();
    super::agents::send_agent_list(state).await;
    deliver(
        state,
        agent,
        SteerSource::Harness,
        carry_on(opening, &asks),
        false,
    )
    .await;
}

/// The opening of the prompt resuming an agent whose sign-in works again.
const SIGNED_IN: &str = "Your model sign-in works again.";

/// The opening of the prompt retrying a turn that failed on `why`.
fn after_error(why: &str) -> String {
    let why = why.trim().trim_end_matches('.');
    format!("Your last turn stopped on an error reaching the model ({why}).")
}

/// The prompt asking an agent to carry on after `opening`, with `asks`
/// (subject and marker each) it owes.
pub(crate) fn carry_on(opening: &str, asks: &[(String, String)]) -> String {
    let mut text = format!("{opening} Carry on where you left off");
    if asks.is_empty() {
        text.push('.');
        return text;
    }
    text.push_str(", you still owe these reports:\n");
    for (subject, marker) in asks {
        text.push_str(&format!("- {marker}\"{subject}\"\n"));
    }
    text.push_str(
        "\nFinish that work, then end your turn with your report: it goes back on its own. If \
         you are blocked, end your turn saying what blocks you.",
    );
    text
}

/// What a chat says while its agent waits for a sign-in, refused for `why`.
pub(crate) fn sign_in_note(why: &str) -> String {
    let why = why.trim().trim_end_matches('.');
    format!(
        "Waiting for sign-in: {why}. It carries on by itself once a model call works again or \
         you sign in (`/login`)."
    )
}

/// What a row and chat say of a turn retried after `why`, retry `attempt`
/// in `wait` ms.
fn retry_note(why: &str, attempt: u32, wait: u64) -> String {
    let why = why.trim().trim_end_matches('.');
    format!(
        "Retrying after an error ({attempt}/{}) in {}: {why}",
        state::MAX_TURN_RETRIES,
        minutes(wait)
    )
}

/// `ms` as whole minutes, at least one: `4 min`.
fn minutes(ms: u64) -> String {
    format!("{} min", (ms / 60_000).max(1))
}
