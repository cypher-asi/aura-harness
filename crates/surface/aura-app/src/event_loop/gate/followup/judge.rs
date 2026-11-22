//! The judge of work no check vouches for: its verdict is the outcome.

use crate::event_loop::agents::AgentSession;
use aura_agent::AgentLoopResult;
use aura_core_types::{OutcomePayload, OutcomeSignal};
use aura_gate::evidence::transcript::reply::final_reply;
use aura_gate::followup::Suspicion;
use aura_gate::judge::turn::{shown_to_judge, Unverified};
use aura_gate::live::{self, Judged};

/// What the judge of unchecked work is shown of an attempt.
pub(crate) struct Unchecked<'a> {
    /// What the user asked
    pub task: &'a str,
    /// The attempt's file changes as a unified diff
    pub diff: &'a str,
    /// How it ended
    pub result: &'a AgentLoopResult,
    /// Where its own messages start in `result`
    pub turn_start: usize,
    pub suspicions: &'a [Suspicion],
}

/// Ask `judge` through `session`'s kernel whether the work of `ended`,
/// shown as `unchecked`, is done; returns the verdict as the attempt's
/// signal (see [`live::judge_unverified`]).
pub(crate) async fn judge_unchecked(
    session: &AgentSession,
    judge: &str,
    ended: &OutcomePayload,
    unchecked: &Unchecked<'_>,
    auth_token: Option<String>,
) -> Option<(OutcomeSignal, Judged)> {
    let messages = &unchecked.result.messages;
    let reply = final_reply(messages);
    let (commands, last_check) =
        shown_to_judge(messages.get(unchecked.turn_start..).unwrap_or_default());
    let shown = Unverified {
        task: unchecked.task,
        diff: unchecked.diff,
        reply: &reply,
        commands: &commands,
        last_check: &last_check,
        suspicions: unchecked.suspicions,
    };
    live::judge_unverified(&session.kernel, judge, ended, &shown, auth_token).await
}
