//! How a hosted agent's turn ends: observing it, and the results and
//! reply text it produces.

use std::sync::Arc;
use std::time::Duration;

use aura_agent::{AgentLoopEvent, AgentLoopResult};
use aura_core_types::{AgentId, SubagentExit, SubagentResult};
use aura_model::Message;
use tokio::sync::mpsc;

use crate::agents::observer::{HostedTurn, TurnObserver};

/// Tell `observer` a turn started and relay its events through it.
/// Returns the sender the turn should use and the relay to drain.
pub(super) fn observe_start(
    observer: Option<&Arc<dyn TurnObserver>>,
    turn: &HostedTurn<'_>,
    events: Option<mpsc::Sender<AgentLoopEvent>>,
) -> (
    Option<mpsc::Sender<AgentLoopEvent>>,
    Option<tokio::task::JoinHandle<()>>,
) {
    match observer {
        Some(observer) => {
            observer.turn_started(turn);
            let (tx, relay) = relay_events(Arc::clone(observer), turn.agent_id, events);
            (Some(tx), Some(relay))
        }
        None => (events, None),
    }
}

/// Tell `observer` how the turn ended, and turn what it produced into
/// its outcome and the history it leaves.
pub(super) fn observe_end(
    observer: Option<&Arc<dyn TurnObserver>>,
    agent_id: AgentId,
    ran: Result<AgentLoopResult, SubagentResult>,
) -> (SubagentResult, Option<Vec<Message>>) {
    match ran {
        Ok(loop_result) => {
            let exit = exit_of(&loop_result);
            if let Some(observer) = observer {
                observer.turn_ended(agent_id, Some(&loop_result), &exit);
            }
            finished(agent_id, loop_result, exit)
        }
        Err(early) => {
            if let Some(observer) = observer {
                observer.turn_ended(agent_id, None, &early.exit);
            }
            (early, None)
        }
    }
}

pub(super) fn reply_text(result: &SubagentResult) -> String {
    match &result.exit {
        SubagentExit::Completed if result.final_message.trim().is_empty() => {
            "(finished without a reply)".to_string()
        }
        SubagentExit::Completed => result.final_message.clone(),
        SubagentExit::Failed { reason } => format!("failed: {reason}"),
        SubagentExit::Cancelled => "was cancelled".to_string(),
        SubagentExit::Timeout => "timed out".to_string(),
        SubagentExit::Rejected { reason } => format!("rejected: {reason}"),
    }
}

/// How a turn that produced `result` ended.
fn exit_of(result: &AgentLoopResult) -> SubagentExit {
    if result.output_truncated || result.insufficient_credits || result.stalled {
        SubagentExit::Failed {
            reason: "child execution stopped before completion".into(),
        }
    } else if result.timed_out {
        SubagentExit::Timeout
    } else {
        result
            .llm_error
            .as_ref()
            .map_or(SubagentExit::Completed, |reason| SubagentExit::Failed {
                reason: reason.clone(),
            })
    }
}

/// The outcome of a turn that produced `result`, and the history it
/// leaves.
fn finished(
    agent_id: AgentId,
    result: AgentLoopResult,
    exit: SubagentExit,
) -> (SubagentResult, Option<Vec<Message>>) {
    let outcome = SubagentResult {
        child_agent_id: Some(agent_id),
        final_message: result.final_text,
        total_input_tokens: result.total_input_tokens,
        total_output_tokens: result.total_output_tokens,
        files_changed: result
            .file_changes
            .into_iter()
            .map(|change| change.path)
            .collect(),
        exit,
    };
    (outcome, Some(result.messages))
}

/// How long the end of a turn waits for its events to reach the
/// observer.
pub(super) const RELAY_DRAIN: Duration = Duration::from_secs(5);

/// A sender for a turn's events that shows each to `observer`, in order,
/// then passes it on to `downstream`. The task ends once every sender is
/// gone.
fn relay_events(
    observer: Arc<dyn TurnObserver>,
    agent_id: AgentId,
    downstream: Option<mpsc::Sender<AgentLoopEvent>>,
) -> (mpsc::Sender<AgentLoopEvent>, tokio::task::JoinHandle<()>) {
    let (tx, mut rx) = mpsc::channel::<AgentLoopEvent>(1024);
    let relay = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            observer.on_event(agent_id, &event);
            if let Some(downstream) = &downstream {
                let _ = downstream.send(event).await;
            }
        }
    });
    (tx, relay)
}

pub(super) fn ended(agent_id: AgentId, exit: SubagentExit) -> SubagentResult {
    SubagentResult {
        child_agent_id: Some(agent_id),
        final_message: String::new(),
        total_input_tokens: 0,
        total_output_tokens: 0,
        files_changed: Vec::new(),
        exit,
    }
}

pub(super) fn cancelled(agent_id: AgentId) -> SubagentResult {
    ended(agent_id, SubagentExit::Cancelled)
}

pub(super) fn failed(agent_id: AgentId, reason: String) -> SubagentResult {
    ended(agent_id, SubagentExit::Failed { reason })
}
