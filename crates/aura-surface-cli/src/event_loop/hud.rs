//! What the terminal's status line and its `/context`, `/cost` and
//! `/todos` commands show, built from what agents report.

use super::agent_events::AgentUi;
use super::agents::AgentSession;
use super::LoopState;
use crate::session_checkpoint::SessionUsage;
use aura_agent::AgentLoopResult;
use aura_core_types::AgentId;
use aura_model_reasoner::{model_catalog, Usage};
use aura_terminal::events::{CallStats, ContextView, JobView, SessionTotals, TodoState, TodoView};
use aura_terminal::UiCommand;
use aura_tools::jobs::JobSummary;
use aura_tools::todo::{TodoItem, TodoStatus};
use std::collections::HashMap;

/// `todos` as the UI shows them.
pub(super) fn todo_views(todos: &[TodoItem]) -> Vec<TodoView> {
    todos
        .iter()
        .map(|todo| TodoView {
            content: todo.content.clone(),
            state: match todo.status {
                TodoStatus::Pending => TodoState::Pending,
                TodoStatus::InProgress => TodoState::InProgress,
                TodoStatus::Completed => TodoState::Completed,
                TodoStatus::Cancelled => TodoState::Cancelled,
            },
        })
        .collect()
}

/// Tokens and cost of one call to `model`.
pub(super) fn call_stats(model: &str, usage: &Usage, context_limit: Option<u64>) -> CallStats {
    let tokens = model_catalog::billed_tokens(model, usage);
    CallStats {
        input_tokens: tokens.uncached_input,
        cache_read_tokens: tokens.cache_read,
        cache_write_tokens: tokens.cache_write,
        output_tokens: tokens.output,
        cost_usd: model_catalog::estimate_cost_usd(model, usage),
        context_limit,
    }
}

/// The make-up of the context a turn ended with.
pub(super) fn context_view(result: &AgentLoopResult, limit: Option<u64>) -> ContextView {
    let breakdown = &result.context_breakdown;
    ContextView {
        system_tokens: breakdown.system_prompt_tokens,
        tools_tokens: breakdown.tools_tokens + breakdown.mcp_tokens + breakdown.subagents_tokens,
        skills_tokens: breakdown.skills_tokens,
        conversation_tokens: breakdown.conversation_tokens,
        total_tokens: result.estimated_context_tokens,
        limit_tokens: limit,
    }
}

fn job_view(job: &JobSummary) -> JobView {
    JobView {
        id: job.job_id.clone(),
        command: job.command.clone(),
        running: job.running,
    }
}

/// Show every agent its background jobs.
pub(super) async fn send_jobs(state: &LoopState<'_>) {
    let mut by_agent: HashMap<AgentId, Vec<JobView>> = HashMap::new();
    for job in state.jobs.list(None) {
        if let Some(agent_id) = job.agent_id {
            by_agent.entry(agent_id).or_default().push(job_view(&job));
        }
    }
    for (agent_id, jobs) in by_agent {
        AgentUi::new(state.commands.clone(), agent_id)
            .send(UiCommand::SetJobs(jobs))
            .await;
    }
}

/// `usage` as the UI shows it.
pub(super) fn session_totals(usage: &SessionUsage) -> SessionTotals {
    SessionTotals {
        calls: usage.calls,
        input_tokens: usage.input_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_write_tokens: usage.cache_write_tokens,
        output_tokens: usage.output_tokens,
        cost_usd: usage.priced.then_some(usage.cost_usd),
        context_tokens: usage.context_tokens,
    }
}

/// Show `session`'s todo list and usage in its chat.
pub(super) async fn send_session(ui: &AgentUi, session: &AgentSession) {
    ui.send(UiCommand::SetTodos(todo_views(&session.todos)))
        .await;
    ui.send(UiCommand::SetSessionTotals(session_totals(&session.usage)))
        .await;
}

/// Show `agent_id` its todo list and usage, when its session is idle here.
pub(super) async fn send_session_of(state: &LoopState<'_>, agent_id: AgentId) {
    if let Some(session) = state.idle.get(&agent_id) {
        send_session(&AgentUi::new(state.commands.clone(), agent_id), session).await;
    }
}

/// Show `agent_id` the project instruction files its turns load, when
/// its kernel is open.
pub(super) async fn send_instructions_of(state: &LoopState<'_>, agent_id: AgentId) {
    if let Some(kernel) = state.kernel_for(agent_id) {
        state
            .hub
            .send_instructions(agent_id, &kernel.agent_workspace())
            .await;
    }
}

/// Show `agent_id` how much work waits for it.
pub(super) async fn send_queued(state: &LoopState<'_>, agent_id: AgentId) {
    let queued = state
        .queued
        .get(&agent_id)
        .map_or(0, std::collections::VecDeque::len);
    AgentUi::new(state.commands.clone(), agent_id)
        .send(UiCommand::SetQueued(queued))
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_cache_reads_are_on_top_of_input() {
        let usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: Some(1_000),
            cache_read_input_tokens: Some(9_000),
        };
        let stats = call_stats("claude-sonnet-4-5", &usage, Some(200_000));
        assert_eq!(stats.input_tokens, 100);
        assert_eq!(stats.cache_read_tokens, 9_000);
        assert_eq!(stats.prompt_tokens(), 10_100);
        assert_eq!(stats.context_limit, Some(200_000));
    }

    #[test]
    fn todo_states_map_one_to_one() {
        let item = |status| TodoItem {
            id: "1".to_string(),
            content: "x".to_string(),
            status,
        };
        let views = todo_views(&[item(TodoStatus::InProgress), item(TodoStatus::Cancelled)]);
        assert_eq!(views[0].state, TodoState::InProgress);
        assert_eq!(views[1].state, TodoState::Cancelled);
    }
}
