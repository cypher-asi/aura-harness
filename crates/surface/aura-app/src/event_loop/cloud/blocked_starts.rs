//! A member whose start on a cloud machine failed for a reason that won't
//! go away by itself (its project doesn't build, it has no project
//! folder) isn't started again for every message sent to it.
//!
//! # Invariant
//!
//! After such a failure the member stays failed. Work sent to it is turned
//! away at once, its sender told why, with no new start, except:
//! - the user sends it a message themselves, which is a retry;
//! - [`RETRY_BLOCKED_AFTER`] has passed since the last try: one start
//!   tries again, which the hub refuses in seconds, without a builder,
//!   while the merge target's commit is the one that failed to build.
//!
//! A start that fails for its machine (killed, out of disk, a timeout, no
//! connection) doesn't block: the next work tries again.

use super::*;
use crate::event_loop::inbox;

/// How long after a blocked start's last try work may try once more.
pub(super) const RETRY_BLOCKED_AFTER: Duration = Duration::from_secs(10 * 60);

/// Who a start is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::event_loop) enum StartCause {
    /// Work sent to the member: its first assignment, mail, a notice
    Work,
    /// A message the user typed to it: a retry, whatever failed before
    User,
    /// The recovery trying once more after a failed start, its work kept
    /// meanwhile ([`crate::event_loop::recover`])
    Retry,
}

/// Why a start failed, and whether that was its machine's doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::event_loop) struct StartFailure {
    /// What went wrong
    pub(in crate::event_loop) why: String,
    /// Whether a new try may well work ([`aura_engine_cloud::exec_errors::is_transient`])
    pub(in crate::event_loop) transient: bool,
}

/// `work`, a placement, given up as failed after `limit`: its error, and
/// whether that was transient. Taking too long is.
pub(super) async fn start_within<T>(
    limit: Duration,
    work: impl std::future::Future<Output = aura_engine_cloud::CloudResult<T>>,
) -> Result<T, StartFailure> {
    match tokio::time::timeout(limit, work).await {
        Ok(Ok(done)) => Ok(done),
        Ok(Err(err)) => Err(StartFailure {
            transient: aura_engine_cloud::exec_errors::is_transient(&err),
            why: err.to_string(),
        }),
        Err(_) => Err(StartFailure {
            why: format!(
                "the machine did not start within {} minutes",
                limit.as_secs() / 60
            ),
            transient: true,
        }),
    }
}

/// Whether `agent` may start now, for `cause`. When it may not, its work
/// was turned away: its sender told, its chat too.
pub(super) async fn may_start(
    state: &mut LoopState<'_>,
    agent: AgentId,
    cause: StartCause,
) -> bool {
    if matches!(cause, StartCause::User | StartCause::Retry) {
        state.hub.status.unblock_start(agent);
        return true;
    }
    let now = aura_core_types::time::now_ms();
    let after = u64::try_from(RETRY_BLOCKED_AFTER.as_millis()).unwrap_or(u64::MAX);
    let Some(why) = state.hub.status.blocked_start(agent, now, after) else {
        return true;
    };
    info!(%agent, "Its last start failed for good; turning work away without a new start");
    let note = blocked_note(&why);
    while state.queued.pop_front(agent).is_some() {}
    crate::event_loop::hud::send_queued(state, agent).await;
    inbox::give_up_all(&state.hub, agent, &note);
    AgentUi::new(state.commands.clone(), agent)
        .send(UiCommand::ShowError(note))
        .await;
    false
}

/// What work turned away from a member whose last start failed, `why`,
/// says.
fn blocked_note(why: &str) -> String {
    let why = why.trim().trim_end_matches('.');
    format!(
        "Not started again: its last start on a cloud machine failed ({why}), and would \
         fail the same way. It tries once more by itself after {} minutes, or right away \
         when you send it a message yourself.",
        RETRY_BLOCKED_AFTER.as_secs() / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_start_that_never_ends_fails_as_transient() {
        let stuck = std::future::pending::<aura_engine_cloud::CloudResult<()>>();
        let failed = start_within(Duration::from_millis(10), stuck).await;
        assert_eq!(
            failed,
            Err(StartFailure {
                why: "the machine did not start within 0 minutes".to_string(),
                transient: true,
            })
        );
        let compile = aura_engine_cloud::CloudError::ExecFailed {
            machine: "b".into(),
            command: "cargo test --workspace --no-run".into(),
            exit_code: 101,
            stderr: "error[E0063]: missing field `run_started`".into(),
        };
        let refused = start_within(START_TIMEOUT, async { Err::<(), _>(compile) }).await;
        assert!(matches!(
            refused,
            Err(StartFailure {
                transient: false,
                ..
            })
        ));
        let started = start_within(START_TIMEOUT, async { Ok(7) }).await;
        assert_eq!(started, Ok(7));
    }
}
