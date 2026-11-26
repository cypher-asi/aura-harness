//! Delegation under Gate: which turns are led, the lead's tools, a
//! task's check and compact result, read-only tasks side by side and
//! write tasks one at a time, the tier a frontier lead's subtasks stay
//! under, the one escalation of a failed subtask, and `/gate`'s
//! delegation figures.
//!
//! - `lead`: which turns are led, and the lead's tools and limits
//! - `check`: a task's check, and the compact result it comes back in
//! - `slots`: read-only tasks side by side, write tasks one at a time
//! - `escalation`: the one retry of a failed subtask on a stronger model,
//!   the judge's say in it, and the tier a frontier lead's subtasks stay
//!   under
//! - `undo`: what of a failed attempt is undone before the retry
//! - `figures`: `/gate`'s delegation figures

mod check;
mod escalation;
mod figures;
mod lead;
mod slots;
mod undo;

use super::*;
use aura_core_types::{SubagentDispatchRequest, SubagentResult};
use aura_model::{ContentBlock, ToolResultContent};

const SONNET: &str = "aura-claude-sonnet-4-6";
const GPT: &str = "aura-gpt-5-4";
const HAIKU: &str = "aura-claude-haiku-4-5";

fn task(parent: AgentId, kind: &str, prompt: &str, check: Option<&str>) -> SubagentDispatchRequest {
    serde_json::from_value(serde_json::json!({
        "parent_agent_id": parent,
        "subagent_type": kind,
        "prompt": prompt,
        "parent_permissions": aura_core_types::AgentPermissions::full_access(),
        "user_tool_defaults": aura_core_types::UserToolDefaults::default(),
        "check": check,
    }))
    .unwrap()
}

async fn dispatch(fixture: &Fixture, request: SubagentDispatchRequest) -> SubagentResult {
    fixture.dispatcher.dispatch(request).await.unwrap()
}

fn select_gate(fixture: &Fixture, state: &mut LoopState<'_>) {
    state.models = gate_on_zos(fixture);
    state.sync_turn_base();
}

fn names(tools: &[ToolDefinition]) -> Vec<&str> {
    tools.iter().map(|t| t.name.as_str()).collect()
}

/// The text of every tool result in `agent_id`'s idle session.
fn tool_results(state: &LoopState<'_>, agent_id: AgentId) -> Vec<(String, bool)> {
    state.idle[&agent_id]
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => Some((
                match content {
                    ToolResultContent::Text(text) => text.clone(),
                    ToolResultContent::Json(value) => value.to_string(),
                },
                *is_error,
            )),
            _ => None,
        })
        .collect()
}

/// The prompts and outcomes in `agent_id`'s record.
fn record(fixture: &Fixture, agent_id: AgentId) -> (Vec<PromptPayload>, Vec<OutcomePayload>) {
    let entries = fixture
        .hub
        .factory
        .store
        .scan_record(agent_id, 1, 10_000)
        .unwrap();
    let prompts = entries.iter().filter_map(|e| e.tx.prompt_payload());
    let outcomes = entries.iter().filter_map(|e| e.tx.outcome_payload());
    (prompts.collect(), outcomes.collect())
}

/// Children of `parent` that ran `kind`, oldest first.
fn children(fixture: &Fixture, parent: AgentId, kind: &str) -> Vec<AgentId> {
    let registry = subagents::lock(&fixture.hub.registry);
    registry
        .descendants(parent)
        .into_iter()
        .filter(|agent| {
            registry
                .find(*agent)
                .is_some_and(|a| a.definition.as_deref() == Some(kind))
        })
        .collect()
}

/// Under Gate, the user's agent, in a turn recorded on Sonnet, dispatches
/// "write a note" checked by `ls notes/{GPT}.txt`: the first attempt, on
/// Sonnet, fails the check, and its retry on GPT passes it. Returns the
/// subtask's result and its check.
async fn retried_once(fixture: &Fixture, state: &mut LoopState<'_>) -> (SubagentResult, String) {
    let first = fixture.first;
    select_gate(fixture, state);
    fixture.hub.set_permissions(first, yolo());
    let lead = state.idle.get(&first).unwrap();
    let mut turn_prompt = PromptPayload::new("make the notes");
    turn_prompt.route = Route::turn(SONNET, RouteReason::Default);
    turns::record_prompt(lead, &turn_prompt).await.unwrap();
    let check = format!("ls notes/{GPT}.txt");
    let result = dispatch(
        fixture,
        task(first, "implement", "write a note", Some(&check)),
    )
    .await;
    (result, check)
}
