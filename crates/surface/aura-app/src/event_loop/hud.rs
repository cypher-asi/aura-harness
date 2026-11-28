//! What the terminal's status line and its `/context`, `/cost` and
//! `/todos` commands show, built from what agents report. Background jobs
//! come from the Processes tab's table (see [`super::processes`]).

use super::agent_events::AgentUi;
use super::agents::AgentSession;
use super::plan_mode::todo_state;
use super::LoopState;
use crate::session_checkpoint::SessionUsage;
use aura_agent::AgentLoopResult;
use aura_core_types::AgentId;
use aura_exec_tools::todo::TodoItem;
use aura_gate::compare::ModelCall;
use aura_ui::events::{CallStats, ContextView, SessionTotals, TodoView};
use aura_ui::UiCommand;

/// `todos` as the UI shows them.
pub(super) fn todo_views(todos: &[TodoItem]) -> Vec<TodoView> {
    todos
        .iter()
        .map(|todo| TodoView {
            content: todo.content.clone(),
            state: todo_state(todo.status),
            assignee: todo.assignee.clone(),
            task: None,
        })
        .collect()
}

/// Tokens and cost of one model call: its usage read in the shape of the
/// endpoint it went to, priced as [`ModelCall`] prices it. A call is
/// unpriced when its model has no known price; a local model's are free.
pub(crate) fn call_stats(call: &ModelCall, context_limit: Option<u64>) -> CallStats {
    let tokens = call.billed_tokens();
    CallStats {
        model: call.model.clone(),
        input_tokens: tokens.uncached_input,
        cache_read_tokens: tokens.cache_read,
        cache_write_tokens: tokens.cache_write,
        output_tokens: tokens.output,
        cost_usd: call.estimate_cost_usd(),
        unpriced: call.list_price().is_none(),
        context_limit,
    }
}

/// The session totals of the one call `stats` reports.
pub(crate) fn call_session_usage(stats: &CallStats) -> SessionUsage {
    SessionUsage {
        calls: 1,
        input_tokens: stats.input_tokens,
        cache_read_tokens: stats.cache_read_tokens,
        cache_write_tokens: stats.cache_write_tokens,
        output_tokens: stats.output_tokens,
        cost_usd: stats.cost_usd.unwrap_or(0.0),
        priced: stats.cost_usd.is_some(),
        unpriced_calls: u32::from(stats.unpriced),
        context_tokens: stats.prompt_tokens(),
        models: vec![stats.model.clone()],
    }
}

/// The make-up of the context a turn ended with.
pub(super) fn context_view(result: &AgentLoopResult, limit: Option<u64>) -> ContextView {
    let breakdown = &result.context_breakdown;
    ContextView {
        system_tokens: breakdown.system_prompt_tokens,
        tools_tokens: breakdown.tools_tokens,
        mcp_tokens: breakdown.mcp_tokens,
        subagents_tokens: breakdown.subagents_tokens,
        skills_tokens: breakdown.skills_tokens,
        conversation_tokens: breakdown.conversation_tokens,
        total_tokens: result.estimated_context_tokens,
        limit_tokens: limit,
    }
}

/// The make-up of the context a cloud turn ended with, as its machine
/// reported it, measured against `limit`.
pub(super) fn remote_context_view(
    usage: &aura_core_protocol::SessionUsage,
    limit: Option<u64>,
) -> ContextView {
    let breakdown = &usage.context_breakdown;
    ContextView {
        system_tokens: breakdown.system_prompt_tokens,
        tools_tokens: breakdown.tools_tokens,
        mcp_tokens: breakdown.mcp_tokens,
        subagents_tokens: breakdown.subagents_tokens,
        skills_tokens: breakdown.skills_tokens,
        conversation_tokens: breakdown.conversation_tokens,
        total_tokens: usage.estimated_context_tokens,
        limit_tokens: limit,
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
        unpriced_calls: usage.unpriced_calls,
        context_tokens: usage.context_tokens,
        models: usage.models.clone(),
    }
}

/// Show `session`'s todo list and usage in its chat.
pub(super) async fn send_session(ui: &AgentUi, session: &AgentSession) {
    ui.send(UiCommand::SetTodos(todo_views(&session.todos)))
        .await;
    ui.send(UiCommand::SetSessionTotals(session_totals(&session.usage)))
        .await;
}

/// Show `agent_id` its todo list and usage, when its session is idle here
/// or, for a cloud agent, from the copy of its machine's session.
pub(super) async fn send_session_of(state: &LoopState<'_>, agent_id: AgentId) {
    let ui = AgentUi::new(state.commands.clone(), agent_id);
    if let Some(session) = state.idle.get(&agent_id) {
        send_session(&ui, session).await;
    } else if state.cloud.is_cloud(agent_id) {
        let Some(sessions) = state.hub.factory.checkpoints.clone() else {
            return;
        };
        let copied = tokio::task::spawn_blocking(move || sessions.load_current(agent_id)).await;
        if let Ok(Some(checkpoint)) = copied {
            ui.send(UiCommand::SetTodos(todo_views(&checkpoint.todos)))
                .await;
            ui.send(UiCommand::SetSessionTotals(session_totals(
                &checkpoint.usage,
            )))
            .await;
        }
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

/// Show `agent_id` the work that waits for it. Its peers see how much as
/// the queue changes ([`super::queue::WorkQueues::counted`]).
pub(super) async fn send_queued(state: &LoopState<'_>, agent_id: AgentId) {
    AgentUi::new(state.commands.clone(), agent_id)
        .send(UiCommand::SetQueue(state.queued.views(agent_id)))
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_model::Usage;

    fn routed(model: &str, route: Option<&str>, usage: Usage) -> ModelCall {
        ModelCall {
            model: model.to_string(),
            route: route.map(str::to_string),
            purpose: None,
            usage,
        }
    }

    #[test]
    fn anthropic_cache_reads_are_on_top_of_input() {
        let usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: Some(1_000),
            cache_read_input_tokens: Some(9_000),
        };
        let stats = call_stats(&routed("claude-sonnet-4-5", None, usage), Some(200_000));
        assert_eq!(stats.input_tokens, 100);
        assert_eq!(stats.cache_read_tokens, 9_000);
        assert_eq!(stats.prompt_tokens(), 10_100);
        assert_eq!(stats.context_limit, Some(200_000));
    }

    #[test]
    fn a_call_is_read_and_priced_by_the_route_it_took() {
        // OpenRouter answers a GPT model in Anthropic shape, naming it by
        // its bare slug.
        let usage = Usage::new(1_000, 50).with_cache(None, Some(800));
        let stats = call_stats(
            &routed(
                "openai/gpt-5.4",
                Some("openrouter/openai/gpt-5.4"),
                usage.clone(),
            ),
            None,
        );
        assert_eq!(stats.model, "openai/gpt-5.4");
        assert_eq!(stats.input_tokens, 1_000, "cache reads come on top");
        assert!(stats.cost_usd.is_some_and(|cost| cost > 0.0));
        assert!(!stats.unpriced);
        // A local server naming its model like a slug is not priced, but
        // free rather than unpriced.
        let local = routed(
            "openai/gpt-5.4",
            Some("local/openai/gpt-5.4"),
            usage.clone(),
        );
        let local = call_stats(&local, None);
        assert_eq!(local.cost_usd, None);
        assert!(!local.unpriced);
        // A model with no known price is unpriced.
        let unknown = routed("mystery/model-x", Some("openrouter/mystery/model-x"), usage);
        let unknown = call_stats(&unknown, None);
        assert_eq!(unknown.cost_usd, None);
        assert!(unknown.unpriced);
        let mut session = call_session_usage(&stats);
        session.add(&call_session_usage(&unknown));
        assert!(session.priced);
        assert_eq!(session.unpriced_calls, 1);
        let totals = session_totals(&session);
        assert_eq!(totals.unpriced_calls, 1);
        assert!(totals.cost_usd.is_some());
    }

    #[test]
    fn todo_states_map_one_to_one() {
        use aura_exec_tools::todo::TodoStatus;
        use aura_ui::events::TodoState;
        let item = |status| TodoItem {
            id: "1".to_string(),
            content: "x".to_string(),
            status,
            assignee: None,
        };
        let views = todo_views(&[item(TodoStatus::InProgress), item(TodoStatus::Cancelled)]);
        assert_eq!(views[0].state, TodoState::InProgress);
        assert_eq!(views[1].state, TodoState::Cancelled);
    }

    #[test]
    fn a_cloud_turn_s_context_is_shown_as_its_machine_reported_it() {
        let usage = aura_core_protocol::SessionUsage {
            estimated_context_tokens: 9_000,
            context_breakdown: aura_core_protocol::ContextBreakdown {
                system_prompt_tokens: 2_000,
                tools_tokens: 1_000,
                mcp_tokens: 300,
                subagents_tokens: 200,
                skills_tokens: 500,
                conversation_tokens: 5_000,
                ..Default::default()
            },
            ..Default::default()
        };
        let view = remote_context_view(&usage, Some(200_000));
        assert_eq!(view.system_tokens, 2_000);
        // Each its own part of the CONTEXT panel.
        assert_eq!(view.tools_tokens, 1_000);
        assert_eq!(view.mcp_tokens, 300);
        assert_eq!(view.subagents_tokens, 200);
        assert_eq!(view.skills_tokens, 500);
        assert_eq!(view.conversation_tokens, 5_000);
        assert_eq!(view.total_tokens, 9_000);
        assert_eq!(view.limit_tokens, Some(200_000));
    }
}
