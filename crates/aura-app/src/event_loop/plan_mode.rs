//! Plans in the terminal: the plan strip, and `/build`.
//!
//! `/plan` is the `plan` permission mode (see [`super::modes`]): the
//! kernel holds the agent to reads and markdown notes, and its turns get
//! the plan-mode instructions, so it researches and writes a plan with
//! `create_plan`. `/plan` again leaves that mode and returns to the one
//! from before. `/build` switches the agent back to a mode that edits,
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
use aura_tools::plan_tools::{is_plan_path, todo_items, PlanFile, TodoStatus, PLANS_DIR};
use std::path::Path;
use std::time::SystemTime;
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

/// Every readable plan in `workspace`, most recently changed first.
pub(crate) fn plan_views(workspace: &Path) -> Vec<PlanView> {
    let Ok(entries) = std::fs::read_dir(workspace.join(PLANS_DIR)) else {
        return Vec::new();
    };
    let mut plans: Vec<(SystemTime, PlanView)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = format!("{PLANS_DIR}/{}", entry.file_name().to_str()?);
            let changed = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            Some((changed, plan_view(workspace, &path)?))
        })
        .collect();
    plans.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.path.cmp(&b.1.path)));
    plans.into_iter().map(|(_, view)| view).collect()
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

/// Show the Plans tab every plan in `agent_id`'s workspace.
pub(super) async fn send_plans_of(state: &LoopState<'_>, agent_id: AgentId) {
    let Some(kernel) = state.kernel_for(agent_id) else {
        return;
    };
    let plans = plan_views(&kernel.agent_workspace());
    AgentUi::new(state.commands.clone(), agent_id)
        .send(UiCommand::SetPlans(plans))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn write_plan(workspace: &Path, file: &str, name: &str, age_secs: u64) {
        let path = workspace.join(PLANS_DIR).join(file);
        let text = format!(
            "---\nname: {name}\ntodos:\n  - id: a\n    content: First\n    status: completed\n---\n"
        );
        std::fs::write(&path, text).unwrap();
        let changed = SystemTime::now() - Duration::from_secs(age_secs);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(changed)
            .unwrap();
    }

    #[test]
    fn plan_views_lists_readable_plans_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        assert!(plan_views(dir.path()).is_empty());

        std::fs::create_dir_all(dir.path().join(PLANS_DIR)).unwrap();
        write_plan(dir.path(), "old_00000001.plan.md", "Old", 300);
        write_plan(dir.path(), "new_00000002.plan.md", "New", 10);
        std::fs::write(dir.path().join(PLANS_DIR).join("notes.md"), "x").unwrap();
        std::fs::write(
            dir.path().join(PLANS_DIR).join("broken_00000003.plan.md"),
            "no frontmatter",
        )
        .unwrap();

        let plans = plan_views(dir.path());
        let names: Vec<&str> = plans.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["New", "Old"]);
        assert_eq!(plans[0].path, ".aura/plans/new_00000002.plan.md");
        assert_eq!(plans[0].todos[0].state, TodoState::Completed);
    }
}
