//! Session modes and plans in the terminal: `/mode`, `/plan`, `/build`.
//!
//! The mode belongs to the terminal, not to one agent: switching it goes
//! through the active agent's kernel (which records a `mode_change` entry
//! and enforces the new mode from then on), and an agent opened later is
//! switched when it becomes active.

use super::handlers::show_system_message;
use super::LoopState;
use aura_context_prompts::model_messages::build_plan::build_plan_prompt;
use aura_core_types::AgentMode;
use aura_terminal::events::{MessageData, MessageRole, PlanView, TodoState, TodoView};
use aura_terminal::UiCommand;
use aura_tools::plan_tools::{is_plan_path, PlanFile, TodoStatus};
use std::path::Path;
use tracing::{info, warn};

/// The plan at `path` (relative to `workspace`) as the UI shows it, or
/// `None` when it can't be read.
pub(crate) fn plan_view(workspace: &Path, path: &str) -> Option<PlanView> {
    if !is_plan_path(path) {
        return None;
    }
    match PlanFile::load(&workspace.join(path)) {
        Ok(plan) => Some(PlanView {
            path: path.to_string(),
            name: plan.meta.name,
            todos: plan
                .meta
                .todos
                .into_iter()
                .map(|todo| TodoView {
                    content: todo.content,
                    status: todo_state(todo.status),
                })
                .collect(),
        }),
        Err(e) => {
            warn!(%path, error = %e, "Could not read plan");
            None
        }
    }
}

const fn todo_state(status: TodoStatus) -> TodoState {
    match status {
        TodoStatus::Pending => TodoState::Pending,
        TodoStatus::InProgress => TodoState::InProgress,
        TodoStatus::Completed => TodoState::Completed,
        TodoStatus::Cancelled => TodoState::Cancelled,
    }
}

/// Put the active agent in the session mode, recording the change when
/// its kernel was in another mode (the record observer shows the entry),
/// and tell the UI. Reports failures itself and returns `false`.
pub(super) async fn apply_mode(state: &mut LoopState<'_>, mode: AgentMode) -> bool {
    if let Err(e) = state.session.kernel.switch_mode(mode).await {
        warn!(error = %e, ?mode, "Mode switch failed");
        let _ = state
            .commands
            .send(UiCommand::ShowError(format!("Could not switch mode: {e}")))
            .await;
        return false;
    }
    state.mode = mode;
    if let Some(agents) = &state.hub.agents {
        agents.set_mode(mode);
    }
    let _ = state.commands.send(UiCommand::SetMode(mode)).await;
    true
}

/// Show the active agent's plan, or hide the plan strip when it has none.
pub(super) async fn send_active_plan(state: &LoopState<'_>) {
    let workspace = state.session.kernel.agent_workspace();
    let view = state
        .session
        .active_plan
        .as_deref()
        .and_then(|path| plan_view(&workspace, path));
    let _ = state.commands.send(UiCommand::SetPlan(view)).await;
}

/// `/mode`, `/plan`, Shift+Tab.
pub(super) async fn handle_set_mode(state: &mut LoopState<'_>, mode: AgentMode) {
    info!(?mode, "Switching session mode");
    if !apply_mode(state, mode).await {
        return;
    }
    let note = match mode {
        AgentMode::Plan => {
            "Plan mode: the agent researches and writes a plan instead of changing code. \
             Run /build when the plan looks right."
        }
        AgentMode::Agent => "Agent mode: the agent can edit files and run commands.",
        AgentMode::Ask => "Ask mode: read-only answers, no edits or commands.",
        AgentMode::Debug => "Debug mode: reads and sandboxed probes, no edits.",
    };
    show_system_message(state, note.to_string()).await;
}

/// `/build [path]`: switch to agent mode and have the agent implement the
/// plan at `path`, or the active agent's latest plan.
pub(super) async fn handle_build_plan(state: &mut LoopState<'_>, path: Option<String>) {
    let Some(path) = path.or_else(|| state.session.active_plan.clone()) else {
        let _ = state
            .commands
            .send(UiCommand::ShowWarning(
                "No plan to build yet. Switch to plan mode with /plan and ask for one.".into(),
            ))
            .await;
        return;
    };
    let workspace = state.session.kernel.agent_workspace();
    let Some(view) = plan_view(&workspace, &path) else {
        let _ = state
            .commands
            .send(UiCommand::ShowError(format!(
                "Could not read plan {path} (plans live in .aura/plans/*.plan.md)"
            )))
            .await;
        return;
    };
    if !apply_mode(state, AgentMode::Agent).await {
        return;
    }
    info!(%path, "Building plan");
    show_system_message(state, format!("Agent mode: building {}", view.name)).await;
    state.session.active_plan = Some(path.clone());
    let _ = state.commands.send(UiCommand::SetPlan(Some(view))).await;

    let prompt = build_plan_prompt(&path);
    let _ = state
        .commands
        .send(UiCommand::ShowMessage(MessageData::new(
            MessageRole::User,
            prompt.clone(),
        )))
        .await;
    let _ = state.commands.send(UiCommand::TurnStarted).await;
    super::handle_user_message(state, prompt).await;
}
