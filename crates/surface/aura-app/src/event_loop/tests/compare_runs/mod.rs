//! `/compare`: both arms in separate copies, what each run records, how
//! each arm is routed, picking and applying a run, the budget, and the
//! BENCH tab's replayed sessions and example tasks.
//!
//! - `asking`: what `/compare` needs before it starts, and what it refuses
//! - `running`: both arms run repeatedly in copies of their own, and what
//!   each run records, traces and shows
//! - `judging`: the judge's verdicts on the pairs
//! - `applying`: a run's diff, and bringing the chosen run home
//! - `stopping`: what stops a comparison: its budget, or Aura quitting
//!   while it runs
//! - `placement`: where a comparison goes among the agents, and the name it
//!   gives the agent that asked for it
//! - `routing`: how each arm routes its turns and subtasks: on a pinned
//!   model, by difficulty, or by Gate
//! - `escalation`: a Gate run that fails its checks carrying on on a
//!   stronger model
//! - `gate_isolation`: each arm routing, its subtasks too, on a detached
//!   view of Gate drawing from a generator of its own
//! - `scratch`: each run's commands get temp, cache and build folders of
//!   their own beside its copy, which its subtasks share
//! - `sessions`: a session replayed prompt by prompt from the BENCH tab
//! - `examples`: example tasks run from the BENCH tab, each in a fixture
//!   of its own with a check of its own
//! - `pins`: one comparison whose every word and record is pinned byte for
//!   byte

#![deny(clippy::too_many_lines)]

mod applying;
mod asking;
mod cache_salt;
mod escalation;
mod examples;
mod gate_isolation;
mod judging;
mod pins;
mod placement;
mod routing;
mod running;
mod scratch;
mod sessions;
mod stopping;

use super::*;
use aura_core_types::{ArmSpec, CompareArm, ComparePayload, CompareRun, CompareStatus};
use aura_ui_terminal::events::{
    BenchExample, BenchRequest, BenchSession, BenchSpec, BenchView, BenchWork,
};

const SONNET: &str = "aura-claude-sonnet-4-6";
const OPUS: &str = "aura-claude-opus-4-7";
const GPT: &str = "aura-gpt-5-4";
const HAIKU: &str = "aura-claude-haiku-4-5";

/// Send `/compare <args>` from the first agent's chat.
async fn compare(state: &mut LoopState<'_>, links: &mpsc::Sender<auth::LinkOutcome>, args: &str) {
    let event = UiEvent::Compare {
        agent_id: state.active.to_hex(),
        args: args.to_string(),
    };
    handle_ui_event(state, event, links).await;
}

/// Prepare and start the comparison `args` asks for, and wait for it to
/// end.
pub(super) async fn run_comparison(
    state: &mut LoopState<'_>,
    links: &mpsc::Sender<auth::LinkOutcome>,
    commands: &mut mpsc::Receiver<UiCommand>,
    args: &str,
) -> ComparePayload {
    compare(state, links, args).await;
    let estimate = system_texts(&drain(commands));
    assert!(estimate.contains("/compare go starts it"), "{estimate}");
    go(state, links).await
}

/// Start the comparison prepared in the first agent's chat, and wait for
/// it to end.
async fn go(state: &mut LoopState<'_>, links: &mpsc::Sender<auth::LinkOutcome>) -> ComparePayload {
    compare(state, links, "go").await;
    tokio::time::timeout(Duration::from_secs(60), state.compare.finished())
        .await
        .expect("the comparison did not end");
    latest(state)
}

/// The latest state of the latest comparison.
fn latest(state: &LoopState<'_>) -> ComparePayload {
    let host = subagents::lock(&state.hub.registry)
        .compares()
        .last()
        .map(|agent| agent.agent_id())
        .expect("a comparison is registered");
    entries(state, host)
        .iter()
        .rev()
        .find_map(|entry| entry.tx.compare_payload())
        .expect("the comparison is recorded")
}

/// A record entry scan that works for agents with no open kernel too.
pub(super) fn entries(
    state: &LoopState<'_>,
    agent_id: AgentId,
) -> Vec<aura_core_types::RecordEntry> {
    state
        .hub
        .factory
        .store
        .scan_record(agent_id, 1, 10_000)
        .unwrap()
}

fn signals_of(state: &LoopState<'_>, agent_id: AgentId) -> Vec<OutcomeSignal> {
    entries(state, agent_id)
        .iter()
        .filter_map(|e| e.tx.outcome_payload())
        .map(|o| o.signal)
        .collect()
}

/// The `/compare` check proposed in `entry`, if it is one.
fn check_proposal(entry: &aura_core_types::RecordEntry) -> Option<aura_core_types::ToolProposal> {
    if entry.tx.tx_type != aura_core_types::TransactionType::ToolProposal {
        return None;
    }
    serde_json::from_slice::<aura_core_types::ToolProposal>(&entry.tx.payload)
        .ok()
        .filter(|p| p.source.as_deref() == Some("compare_check") && p.tool == "run_command")
}

/// Send `request` from the first agent's BENCH tab.
async fn bench(
    state: &mut LoopState<'_>,
    links: &mpsc::Sender<auth::LinkOutcome>,
    request: BenchRequest,
) {
    let event = UiEvent::Bench {
        agent_id: state.active.to_hex(),
        request,
    };
    handle_ui_event(state, event, links).await;
}

/// The estimate the BENCH tab was sent, once `commands` are drained.
fn bench_estimate(commands: &mut mpsc::Receiver<UiCommand>) -> Option<Result<String, String>> {
    drain(commands).into_iter().find_map(|(_, c)| match c {
        UiCommand::BenchEstimate { estimate, .. } => Some(estimate),
        _ => None,
    })
}

/// The benchmarks the BENCH tab was sent and the root of their tree, once
/// `commands` are drained.
fn bench_list(commands: &mut mpsc::Receiver<UiCommand>) -> Option<(Vec<BenchView>, String)> {
    drain(commands).into_iter().find_map(|(_, c)| match c {
        UiCommand::BenchList { benches, root, .. } => Some((benches, root)),
        _ => None,
    })
}

/// The sessions and example tasks the BENCH tab was sent, once `commands`
/// are drained.
fn bench_sessions(
    commands: &mut mpsc::Receiver<UiCommand>,
) -> Option<(Vec<BenchSession>, Vec<BenchExample>)> {
    drain(commands).into_iter().find_map(|(_, c)| match c {
        UiCommand::BenchSessions {
            sessions, examples, ..
        } => Some((sessions, examples)),
        _ => None,
    })
}
