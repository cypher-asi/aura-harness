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

use super::placement::{place, place_request};
use super::*;
use crate::event_loop::inbox;
use std::future::Future;

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

/// `work`, given up as failed after `limit`: its error as text, or that
/// it took too long.
pub(super) async fn within<T, E: std::fmt::Display>(
    limit: Duration,
    work: impl Future<Output = Result<T, E>>,
) -> Result<T, String> {
    match tokio::time::timeout(limit, work).await {
        Ok(result) => result.map_err(|err| err.to_string()),
        Err(_) => Err(format!(
            "the machine did not start within {} minutes",
            limit.as_secs() / 60
        )),
    }
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
) -> bool {
    // A channel without a project it can use keeps its members in the
    // launch project, whose folder is not theirs to take to a machine.
    if lock(&state.hub.hidden_members).contains(&agent) {
        fail_cloud_start(state, agent, NO_PROJECT_FOLDER, None).await;
        return false;
    }
    state.hub.status.set_starting(agent, true);
    state.hub.send_agent_list().await;
    let ui = AgentUi::new(state.commands.clone(), agent);
    let console = BootConsoleUi::new(ui, "Starting in the cloud");
    let why = match connect(state, Some(&console)).await {
        Ok(hub) => match place_request(state, &hub, agent) {
            Some(request) => {
                let mark = MoveMark::take(&state.cloud.shared, agent);
                place(state, hub, request, console, (mark, Start::Member));
                return true;
            }
            None => NO_PROJECT_FOLDER.to_string(),
        },
        Err(why) => why,
    };
    fail_cloud_start(state, agent, &why, Some(&console)).await;
    false
}

/// After `agent`'s move ended: whether it was a new member's start that
/// failed, which then fails the member. The start is over either way.
pub(in crate::event_loop) async fn settle_start(state: &mut LoopState<'_>, agent: AgentId) -> bool {
    state.hub.status.set_starting(agent, false);
    let failed = state.cloud.shared.failed_starts.lock().remove(&agent);
    let Some(why) = failed else {
        return false;
    };
    fail_cloud_start(state, agent, &why, None).await;
    true
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
        self.shared
            .failed_starts
            .lock()
            .insert(agent, why.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_start_that_never_ends_fails_after_its_limit() {
        let stuck = std::future::pending::<Result<(), String>>();
        let failed = within(Duration::from_millis(10), stuck).await;
        assert_eq!(
            failed,
            Err("the machine did not start within 0 minutes".to_string())
        );
        let refused = within(START_TIMEOUT, async { Err::<(), _>("boxd said no") }).await;
        assert_eq!(refused, Err("boxd said no".to_string()));
        let started = within(START_TIMEOUT, async { Ok::<_, String>(7) }).await;
        assert_eq!(started, Ok(7));
    }

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
