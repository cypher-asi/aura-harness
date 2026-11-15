//! Plans in the terminal: the plan strip, and `/build`.
//!
//! `/plan` is the `plan` permission mode (see [`super::modes`]): the
//! kernel holds the agent to reads and markdown notes, and its turns get
//! the plan-mode instructions, so it researches and writes a plan with
//! `create_plan`. `/build` switches the agent back to a mode that edits,
//! seeds its todo list with the plan's todos, and asks it to implement
//! the plan; while it builds, its todo list is mirrored into the plan
//! file (see [`super::agent_events`]).

use super::agent_events::AgentUi;
use super::agents::AgentSession;
use super::handlers::system_message;
use super::hud::todo_views;
use super::modes;
use super::LoopState;
use aura_context_prompts::model_messages::build_plan::build_plan_prompt;
use aura_core_types::AgentId;
use aura_terminal::events::{PlanView, TodoState, TodoView};
use aura_terminal::UiCommand;
use aura_tools::plan_tools::{is_plan_path, todo_items, PlanFile, TodoStatus};
use std::path::Path;
use tracing::{info, warn};

/// The plan at `path` (relative to `workspace`) as the UI shows it, or
/// `None` when it can't be read.
pub(crate) fn plan_view(workspace: &Path, path: &str) -> Option<PlanView> {
    if !is_plan_path(path) {
        return None;
    }
    match PlanFile::load(&workspace.join(path)) {
        Ok(plan) => Some(view_of(path, plan)),
        Err(e) => {
            warn!(%path, error = %e, "Could not read plan");
            None
        }
    }
}

fn view_of(path: &str, plan: PlanFile) -> PlanView {
    PlanView {
        path: path.to_string(),
        name: plan.meta.name,
        todos: plan
            .meta
            .todos
            .into_iter()
            .map(|todo| TodoView {
                content: todo.content,
                state: todo_state(todo.status),
            })
            .collect(),
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

/// Show `agent_id` its plan, or hide its plan strip when it has none.
/// Nothing is sent while the agent works: its turn keeps the strip live.
pub(super) async fn send_plan_of(state: &LoopState<'_>, agent_id: AgentId) {
    let Some(session) = state.idle.get(&agent_id) else {
        return;
    };
    let view = session
        .active_plan
        .as_deref()
        .and_then(|path| plan_view(&session.kernel.agent_workspace(), path));
    AgentUi::new(state.commands.clone(), agent_id)
        .send(UiCommand::SetPlan(view))
        .await;
}

/// Get `session` ready to build the plan at `path`, or its latest plan:
/// switch it to a mode that edits, make the plan's todos its todo list,
/// and show the plan. Returns the prompt that starts the build, or `None`
/// after saying why it can't.
pub(super) async fn start_build(
    state: &mut LoopState<'_>,
    session: &mut AgentSession,
    path: Option<String>,
) -> Option<String> {
    let agent_id = session.agent_id();
    let ui = AgentUi::new(state.commands.clone(), agent_id);
    let Some(path) = path.or_else(|| session.active_plan.clone()) else {
        ui.send(UiCommand::ShowWarning(
            "No plan to build yet. Switch to plan mode with /plan and ask for one.".into(),
        ))
        .await;
        return None;
    };
    let workspace = session.kernel.agent_workspace();
    let plan = match is_plan_path(&path)
        .then(|| PlanFile::load(&workspace.join(&path)))
        .transpose()
    {
        Ok(Some(plan)) => plan,
        Ok(None) | Err(_) => {
            ui.send(UiCommand::ShowError(format!(
                "Could not read plan {path} (plans live in .aura/plans/*.plan.md)"
            )))
            .await;
            return None;
        }
    };
    let mode = state.hub.build_mode(agent_id);
    if !modes::switch_mode(state, agent_id, &session.kernel, mode).await {
        return None;
    }
    info!(%agent_id, %path, "Building plan");
    session.todos = todo_items(&plan);
    session.active_plan = Some(path.clone());
    session.building_plan = true;
    let view = view_of(&path, plan);
    ui.send(system_message(format!("Building {}", view.name)))
        .await;
    ui.send(UiCommand::SetTodos(todo_views(&session.todos)))
        .await;
    ui.send(UiCommand::SetPlan(Some(view))).await;
    Some(build_plan_prompt(&path))
}
