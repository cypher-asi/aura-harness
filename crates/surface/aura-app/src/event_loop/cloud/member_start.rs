//! A new member of a channel whose agents run in the cloud starting on a
//! machine of its own.
//!
//! # Invariant
//!
//! Such a member only ever works in the cloud. When its machine can't be
//! had (no project folder to start from, no connection, a failed or
//! too-slow start), it doesn't fall back to this computer: it fails. Its
//! row shows the failure in red, its chat says why, the work queued for
//! it is dropped, and whoever assigned it work is told at once, with
//! `ok=false` and the reason.

use super::blocked_starts::may_start;
use super::placement::{place, place_request};
use super::*;
use crate::event_loop::inbox;

/// Longest a machine may take to start: past it the start fails instead
/// of leaving the agent moving, its work waiting, forever. A fresh
/// machine usually starts in a few minutes; this leaves room for a slow
/// one but not for a stuck one.
pub(super) const START_TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// Why a new member of a cloud channel can't start when its project gives
/// a machine nothing to start from.
const NO_PROJECT_FOLDER: &str =
    "the channel has no project folder for a cloud machine to start from";

/// How a placement was asked for, which sets what its failure means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Start {
    /// The user moved an agent: a failure leaves it here, where it was.
    Move,
    /// A new member of a cloud channel starts: a failure fails it.
    Member,
}

/// Start `agent`, a new member of a channel whose agents run in the
/// cloud, on a cloud machine of its own before its first work, which
/// waits in its queue meanwhile. Returns whether the start is under way;
/// when it couldn't begin, the member failed ([`fail_cloud_start`]) and
/// the work must not run. Its project's default placement stays as it
/// is: the channel, not the project, puts it there.
pub(in crate::event_loop) async fn start_in_cloud(
    state: &mut LoopState<'_>,
    agent: AgentId,
    cause: StartCause,
) -> bool {
    if !may_start(state, agent, cause).await {
        return false;
    }
    // The user asking for a start gives it its retries afresh.
    if cause == StartCause::User {
        crate::event_loop::recover::started(state, agent);
    }
    // A channel without a project it can use keeps its members in the
    // launch project, whose folder is not theirs to take to a machine.
    if lock(&state.hub.hidden_members).contains(&agent) {
        fail_cloud_start(state, agent, NO_PROJECT_FOLDER, None).await;
        let now = aura_core_types::time::now_ms();
        state.hub.status.block_start(agent, NO_PROJECT_FOLDER, now);
        return false;
    }
    // A new start: an earlier failure is behind it from now, on its rows
    // as for its peers.
    state.hub.status.begin_start(agent);
    state.hub.send_agent_list().await;
    let ui = AgentUi::new(state.commands.clone(), agent);
    let console = BootConsoleUi::new(ui, "Starting in the cloud")
        .tracking_start(Arc::clone(&state.hub), agent);
    let why = match connect(state, Some(&console)).await {
        Ok(hub) => match place_request(state, &hub, agent, true) {
            Some(request) => {
                let mark = MoveMark::take(&state.cloud.shared, agent);
                place(state, hub, request, console, (mark, Start::Member));
                return true;
            }
            None => NO_PROJECT_FOLDER.to_string(),
        },
        Err(why) => why,
    };
    // No cloud to reach (no sign-in or key for it) is not a start a retry
    // fixes: it fails as before.
    fail_cloud_start(state, agent, &why, Some(&console)).await;
    false
}

/// After `agent`'s move ended: whether it was a new member's start that
/// failed, which then fails the member. The start is over either way.
pub(in crate::event_loop) async fn settle_start(state: &mut LoopState<'_>, agent: AgentId) -> bool {
    state.hub.status.set_starting(agent, false);
    let failed = state.cloud.shared.failed_starts.lock().remove(&agent);
    let Some(failure) = failed else {
        state.hub.status.unblock_start(agent);
        crate::event_loop::recover::started(state, agent);
        return false;
    };
    // A start its project failed (it doesn't build) fails again the same
    // way: work sent meanwhile doesn't start it again for nothing.
    if !failure.transient {
        let now = aura_core_types::time::now_ms();
        state.hub.status.block_start(agent, &failure.why, now);
    }
    if !hold_failed_start(state, agent, &failure.why, !failure.transient, None).await {
        fail_cloud_start(state, agent, &failure.why, None).await;
    }
    true
}

/// Hold `agent`, whose start failed because of `why` (its project's doing
/// when `blocked`), for a retry with its work kept, telling its chat and
/// row so. Returns whether it is held: past the retries' cap it is not.
async fn hold_failed_start(
    state: &LoopState<'_>,
    agent: AgentId,
    why: &str,
    blocked: bool,
    console: Option<&BootConsoleUi>,
) -> bool {
    let Some(retry) = crate::event_loop::recover::hold_start(state, agent, why, blocked) else {
        return false;
    };
    let note = format!("{} {retry}", failed_start_note(why));
    state.hub.status.fail(agent, &note);
    let shown = match console {
        Some(console) => console.end(false, note.clone()).await,
        None => false,
    };
    if !shown {
        AgentUi::new(state.commands.clone(), agent)
            .send(UiCommand::ShowWarning(note))
            .await;
    }
    state.hub.send_agent_list().await;
    true
}

/// Start `agent` on a cloud machine again, its last start having failed:
/// the recovery's retry. A start that can't begin is held again, or fails.
pub(in crate::event_loop) async fn retry_start(state: &mut LoopState<'_>, agent: AgentId) {
    if !lock(&state.hub.cloud_members).contains(&agent) || state.cloud.is_cloud(agent) {
        // It runs somewhere already: its queue goes on there.
        crate::event_loop::start_next(state, agent).await;
        return;
    }
    start_in_cloud(state, agent, StartCause::Retry).await;
}

/// `agent`, a new member of a cloud channel, could not start on a cloud
/// machine because of `why`: fail it. It never works here instead.
pub(in crate::event_loop) async fn fail_cloud_start(
    state: &mut LoopState<'_>,
    agent: AgentId,
    why: &str,
    console: Option<&BootConsoleUi>,
) {
    let note = failed_start_note(why);
    state.hub.status.fail(agent, &note);
    while state.queued.pop_front(agent).is_some() {}
    crate::event_loop::hud::send_queued(state, agent).await;
    state.hub.factory_flows.leave(agent);
    inbox::give_up_all(&state.hub, agent, &note);
    let shown = match console {
        Some(console) => console.end(false, note.clone()).await,
        None => false,
    };
    if !shown {
        AgentUi::new(state.commands.clone(), agent)
            .send(UiCommand::ShowError(note))
            .await;
    }
    state.hub.send_agent_list().await;
}

/// What a member that could not start on a cloud machine because of `why`
/// says, in its chat and to whoever assigned it work.
fn failed_start_note(why: &str) -> String {
    let why = why.trim().trim_end_matches('.');
    format!(
        "Could not start on a cloud machine: {why}. This channel's agents run only in the \
         cloud, so it did not start on this computer instead."
    )
}

#[cfg(test)]
impl CloudState {
    /// Hold `agent` as moving, as a start under way does.
    pub(in crate::event_loop) fn begin_test_move(&self, agent: AgentId) {
        self.shared.moving.lock().insert(agent);
    }

    /// Note that `agent`'s start failed because of `why`, as a failed or
    /// too-slow placement does before its move ends.
    pub(in crate::event_loop) fn fail_test_start(&self, agent: AgentId, why: &str) {
        self.fail_test_start_as(agent, why, true);
    }

    /// [`Self::fail_test_start`], for its machine's sake when `transient`,
    /// else for good (its project doesn't build).
    pub(in crate::event_loop) fn fail_test_start_as(
        &self,
        agent: AgentId,
        why: &str,
        transient: bool,
    ) {
        let failure = StartFailure {
            why: why.to_string(),
            transient,
        };
        self.shared.failed_starts.lock().insert(agent, failure);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_note_says_why_and_that_it_did_not_run_here() {
        let note = failed_start_note(" boxd machine limit reached: 4 of 4 machines in use. ");
        assert_eq!(
            note,
            "Could not start on a cloud machine: boxd machine limit reached: 4 of 4 machines \
             in use. This channel's agents run only in the cloud, so it did not start on this \
             computer instead."
        );
    }
}
