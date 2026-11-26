//! Plans in the terminal: the plan strip, `/build`, and the open plan.
//!
//! `/plan` is the `plan` permission mode (see [`super::modes`]): the
//! kernel holds the agent to reads and markdown notes, and its turns get
//! the plan-mode instructions, so it researches and writes a plan with
//! `create_plan`. `/plan` again leaves that mode and returns to the one
//! from before. `/build` switches the agent back to a mode that edits,
//! seeds its todo list with the plan's todos, and asks it to implement
//! the plan; while it builds, its todo list is mirrored into the plan
//! file (see [`super::agent_events`]).
//!
//! The plan-ready note invites the user to say "implement", so a typed
//! message that is only that (see [`asks_to_build`]) builds the agent's
//! plan exactly as `/build` does, as long as it has one it is not already
//! building.
//!
//! An agent's plan is its context, in plan mode or not, for as long as
//! the plan is open ([`open_plan`]: a todo is still pending or in
//! progress):
//! - Any other message the user types as a new request, outside plan
//!   mode, while the agent is not building yet, carries on with the plan
//!   ([`continue_open_plan`]): the agent builds it again in the mode it
//!   is in, its todo list is the plan's, and the model is told to keep to
//!   those todos.
//! - In plan mode, each request revises the open plan.
//!
//! Once every todo is closed, the plan no longer is: prompts don't carry
//! on with it and plan-mode requests don't file under it. A build whose
//! todo list closes completes the plan ([`finish_done_todos`]): the agent
//! drops the list and the plan, so the next `/plan` writes a new one.
//!
//! On the task board (see [`super::tasks`] and `turns::begin_task`),
//! every request while the agent builds its plan is the plan's own
//! ([`TaskView::builds`](aura_ui_terminal::events::TaskView::builds)),
//! and a request in plan mode links to the open plan as a task of its
//! own; a `create_plan` call re-links it to the plan it writes.

use super::agent_events::AgentUi;
use super::agents::AgentSession;
use super::handlers::system_message;
use super::hud::todo_views;
use super::modes;
use super::tasks::PlanTasks;
use super::turns::{Prompt, Request};
use super::LoopState;
use aura_context_prompts::model_messages::build_plan::{build_plan_prompt, continue_plan_note};
use aura_core_types::AgentId;
use aura_exec_tools::plan_tools::{
    is_plan_path, sync_todo_list, todo_items, PlanFile, TodoStatus, PLANS_DIR,
};
use aura_exec_tools::todo::TodoItem;
use aura_ui_terminal::events::{PlanView, TodoState, TodoView};
use aura_ui_terminal::{PermissionMode, UiCommand};
use std::path::Path;
use std::time::SystemTime;
use tracing::{info, warn};

/// The plan at `path` (relative to `workspace`) as the UI shows it, or
/// `None` when it can't be read.
pub(crate) fn plan_view(workspace: &Path, path: &str) -> Option<PlanView> {
    let plan = plan_file(workspace, path)?;
    Some(view_of(path, plan, &PlanTasks::load(workspace)))
}

/// The plan file at `path` (relative to `workspace`), when `path` names a
/// plan and the file reads.
fn plan_file(workspace: &Path, path: &str) -> Option<PlanFile> {
    if !is_plan_path(path) {
        return None;
    }
    PlanFile::load(&workspace.join(path))
        .inspect_err(|e| warn!(%path, error = %e, "Could not read plan"))
        .ok()
}

/// Every readable plan in `workspace`, most recently changed first.
pub(crate) fn plan_views(workspace: &Path) -> Vec<PlanView> {
    let Ok(entries) = std::fs::read_dir(workspace.join(PLANS_DIR)) else {
        return Vec::new();
    };
    let plans: Vec<(SystemTime, String, PlanFile)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = format!("{PLANS_DIR}/{}", entry.file_name().to_str()?);
            let changed = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let plan = plan_file(workspace, &path)?;
            Some((changed, path, plan))
        })
        .collect();
    if plans.is_empty() {
        return Vec::new();
    }
    // Each plan links its todos to the tasks doing them; the task folder
    // can hold hundreds, so it is read once for all of them.
    let tasks = PlanTasks::load(workspace);
    let mut views: Vec<(SystemTime, PlanView)> = plans
        .into_iter()
        .map(|(changed, path, plan)| (changed, view_of(&path, plan, &tasks)))
        .collect();
    views.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.path.cmp(&b.1.path)));
    views.into_iter().map(|(_, view)| view).collect()
}

/// The plan at `path` in `workspace`: its view among `plans`, read from
/// the workspace already, else read now ([`plan_view`]).
pub(crate) fn plan_view_among(
    plans: &[PlanView],
    workspace: &Path,
    path: &str,
) -> Option<PlanView> {
    plans
        .iter()
        .find(|plan| plan.path == path)
        .cloned()
        .or_else(|| plan_view(workspace, path))
}

fn view_of(path: &str, plan: PlanFile, tasks: &PlanTasks) -> PlanView {
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
                assignee: todo.assignee,
                task: tasks.task(path, &todo.id).map(str::to_string),
            })
            .collect(),
    }
}

pub(super) const fn todo_state(status: TodoStatus) -> TodoState {
    match status {
        TodoStatus::Pending => TodoState::Pending,
        TodoStatus::InProgress => TodoState::InProgress,
        TodoStatus::Completed => TodoState::Completed,
        TodoStatus::Cancelled => TodoState::Cancelled,
    }
}

/// Whether `todos` is the todo list of the plan at `path` (relative to
/// `workspace`): the same todo ids, whatever their order or status.
///
/// `/build` seeds the agent's list from the plan, so a session whose list
/// still has the plan's ids is building it, even after the session was
/// reopened and its in-memory build flag was lost.
pub(crate) fn is_plan_todo_list(workspace: &Path, path: &str, todos: &[TodoItem]) -> bool {
    if todos.is_empty() || !is_plan_path(path) {
        return false;
    }
    PlanFile::load(&workspace.join(path)).is_ok_and(|plan| lists_plan_todos(&plan, todos))
}

/// Whether `todos` holds exactly `plan`'s todo ids, in any order.
fn lists_plan_todos(plan: &PlanFile, todos: &[TodoItem]) -> bool {
    if todos.is_empty() {
        return false;
    }
    let mut plan_ids: Vec<&str> = plan.meta.todos.iter().map(|t| t.id.as_str()).collect();
    let mut list_ids: Vec<&str> = todos.iter().map(|t| t.id.as_str()).collect();
    plan_ids.sort_unstable();
    list_ids.sort_unstable();
    plan_ids == list_ids
}

/// The agent's open plan: `active_plan` (relative to `workspace`) when
/// its file loads and at least one of its todos is pending or in
/// progress.
///
/// An open plan is the agent's context whatever its mode: prompts outside
/// plan mode continue it, and requests in plan mode revise it. Once every
/// todo is closed the plan is done, so new work is no longer filed under
/// it.
pub(crate) fn open_plan(workspace: &Path, active_plan: Option<&str>) -> Option<PlanFile> {
    let path = active_plan.filter(|path| is_plan_path(path))?;
    let plan = PlanFile::load(&workspace.join(path)).ok()?;
    let is_open = plan
        .meta
        .todos
        .iter()
        .any(|todo| matches!(todo.status, TodoStatus::Pending | TodoStatus::InProgress));
    is_open.then_some(plan)
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

/// What the user may type to build the plan, once trimmed, lowercased
/// and without a trailing `.` or `!`.
const BUILD_PHRASES: [&str; 8] = [
    "impl",
    "implement",
    "implement it",
    "implement the plan",
    "implement plan",
    "build",
    "build it",
    "build the plan",
];

/// Whether `prompt` is the user asking to build `active_plan`, the
/// agent's plan, which it is not `building` yet: a typed message that is
/// only one of the [`BUILD_PHRASES`]. A harness notice, a message with
/// images, or one that says more is a prompt for the model.
pub(super) fn asks_to_build(prompt: &Prompt, active_plan: Option<&str>, building: bool) -> bool {
    // Only text the user typed has mentions; the harness writes the rest.
    if !prompt.mentions || !prompt.images.is_empty() || building || active_plan.is_none() {
        return false;
    }
    let text = prompt.text.trim().to_lowercase();
    let text = text.trim_end_matches(['.', '!']).trim_end();
    BUILD_PHRASES.contains(&text)
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
    let view = view_of(&path, plan, &PlanTasks::load(&workspace));
    ui.send(system_message(format!("Building {}", view.name)))
        .await;
    ui.send(UiCommand::SetTodos(todo_views(&session.todos)))
        .await;
    ui.send(UiCommand::SetPlan(Some(view))).await;
    Some(build_plan_prompt(&path))
}

/// `prompt` as it runs for `session`: a message the user typed as a new
/// request, outside plan mode, while the agent has an open plan it is not
/// building yet, carries on with that plan (see [`continue_build`]). It
/// keeps the user's text as its first line, which titles its task, and
/// gets the [`continue_plan_note`] after a blank line; it becomes the
/// plan's request ([`Request::Build`]). Any other prompt is returned as
/// it is.
pub(super) async fn continue_open_plan(
    state: &LoopState<'_>,
    session: &mut AgentSession,
    prompt: Prompt,
) -> Prompt {
    let typed = prompt.mentions && prompt.request == Request::New;
    let planning = state.hub.mode_of(session.agent_id()) == PermissionMode::Plan;
    if !typed || planning || session.building_plan {
        return prompt;
    }
    let Some(path) = session.active_plan.clone() else {
        return prompt;
    };
    if !continue_build(state, session, &path).await {
        return prompt;
    }
    info!(agent_id = %session.agent_id(), %path, "Message continues the open plan");
    Prompt {
        text: format!("{}\n\n{}", prompt.text, continue_plan_note(&path)),
        request: Request::Build,
        ..prompt
    }
}

/// Have `session` build the plan at `path` again, if it is open: its todo
/// list mirrors into the plan file from now on. Unlike [`start_build`]
/// the agent stays in its mode, since the user chose it. A list that is
/// already the plan's is kept, with whatever the agent did to it;
/// otherwise the plan's todos become the list. Returns whether the plan
/// is open.
pub(super) async fn continue_build(
    state: &LoopState<'_>,
    session: &mut AgentSession,
    path: &str,
) -> bool {
    let workspace = session.kernel.agent_workspace();
    let Some(plan) = open_plan(&workspace, Some(path)) else {
        return false;
    };
    let ui = AgentUi::new(state.commands.clone(), session.agent_id());
    session.building_plan = true;
    if !lists_plan_todos(&plan, &session.todos) {
        session.todos = todo_items(&plan);
        ui.send(UiCommand::SetTodos(todo_views(&session.todos)))
            .await;
    }
    let view = view_of(path, plan, &PlanTasks::load(&workspace));
    ui.send(UiCommand::SetPlan(Some(view))).await;
    true
}

/// `todos`, or none once every item is closed: a finished list has
/// nothing left to carry into the next turn or show in the chat.
pub(crate) fn unfinished_todos(todos: Vec<TodoItem>) -> Vec<TodoItem> {
    if todos.iter().all(|todo| todo.status.is_closed()) {
        Vec::new()
    } else {
        todos
    }
}

/// Whether the plan at `path` (relative to `workspace`) is finished: it
/// loads, has todos, and every one of them is closed. A plan with no
/// todos yet is still being written.
pub(crate) fn is_finished_plan(workspace: &Path, path: &str) -> bool {
    is_plan_path(path)
        && PlanFile::load(&workspace.join(path)).is_ok_and(|plan| {
            !plan.meta.todos.is_empty() && plan.meta.todos.iter().all(|t| t.status.is_closed())
        })
}

/// Close out `session`'s todo list once every item on it is closed. The
/// list is dropped, so it no longer shows in the chat or carries into the
/// next turn. When the agent was building its plan, the plan gets the
/// final list, stops being the agent's plan, and is returned as the UI
/// shows it, for [`announce_finished_plan`]; the next `/plan` then writes
/// a new one.
pub(super) fn finish_done_todos(session: &mut AgentSession) -> Option<PlanView> {
    if session.todos.is_empty() || !session.todos.iter().all(|t| t.status.is_closed()) {
        return None;
    }
    let todos = std::mem::take(&mut session.todos);
    if !session.building_plan {
        return None;
    }
    session.building_plan = false;
    let path = session.active_plan.take()?;
    let workspace = session.kernel.agent_workspace();
    // The turn mirrors each `todo_write` into the plan, but todos closed
    // by an agent reporting back may not have been; the plan keeps the
    // list as it ended.
    if let Err(e) = sync_todo_list(&workspace.join(&path), &todos) {
        warn!(%path, error = %e, "Could not update the finished plan's todos");
    }
    info!(agent_id = %session.agent_id(), %path, "Plan complete");
    plan_view(&workspace, &path)
}

/// Tell the user `plan` is complete, and show the plans with it closed
/// and none being worked on.
pub(super) async fn announce_finished_plan(ui: &AgentUi, workspace: &Path, plan: &PlanView) {
    ui.send(system_message(format!("Plan complete: {}", plan.name)))
        .await;
    ui.send(UiCommand::SetPlans(plan_views(workspace))).await;
    ui.send(UiCommand::SetPlan(None)).await;
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

    /// How long the Plans tab takes to read in a workspace whose agents
    /// built plans for a while: 20 plans of 8 todos, and 600 tasks of
    /// about 4 KB each on disk, as `.aura` holds after weeks of use.
    ///
    /// ```text
    /// cargo test -p aura-app --lib plan_views_perf -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "load test; run with --ignored --nocapture"]
    fn plan_views_perf() {
        use super::super::tasks::{TaskRecord, TASKS_DIR};
        use aura_ui_terminal::events::TaskView;

        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(PLANS_DIR)).unwrap();
        std::fs::create_dir_all(dir.path().join(TASKS_DIR)).unwrap();
        let todo_ids: Vec<String> = (0..8).map(|t| format!("todo-{t}")).collect();
        for plan in 0..20 {
            let todos: String = todo_ids
                .iter()
                .map(|id| format!("  - id: {id}\n    content: Do {id}\n    status: pending\n"))
                .collect();
            let text = format!("---\nname: Plan {plan}\ntodos:\n{todos}---\n\nBody.\n");
            let file = format!("plan-{plan}_{plan:08x}.plan.md");
            std::fs::write(dir.path().join(PLANS_DIR).join(file), text).unwrap();
        }
        let words = "Read the record once and derive the rest from it. ".repeat(8);
        for task in 0..600u64 {
            let plan = task % 20;
            let record = TaskRecord {
                view: TaskView {
                    id: format!("{task:016x}"),
                    title: format!("Task {task}"),
                    plan: Some(format!("{PLANS_DIR}/plan-{plan}_{plan:08x}.plan.md")),
                    plan_todo: Some(todo_ids[usize::try_from(task / 20 % 8).unwrap()].clone()),
                    items: todo_ids
                        .iter()
                        .map(|id| TodoView {
                            content: format!("{id}: {words}"),
                            state: TodoState::Completed,
                            assignee: None,
                            task: None,
                        })
                        .collect(),
                    summary: Some(words.clone()),
                    ..TaskView::default()
                },
                created_ms: task,
                tool_use_id: None,
            };
            let json = serde_json::to_vec_pretty(&record).unwrap();
            let path = dir.path().join(TASKS_DIR).join(format!("{task:016x}.json"));
            std::fs::write(path, json).unwrap();
        }

        let start = std::time::Instant::now();
        let plans = plan_views(dir.path());
        let took = start.elapsed();
        println!(
            "plan_views over 20 plans and 600 tasks: {:.1} ms",
            took.as_secs_f64() * 1000.0
        );
        assert_eq!(plans.len(), 20);
        assert!(plans
            .iter()
            .all(|plan| plan.todos.iter().all(|todo| todo.task.is_some())));
    }

    #[test]
    fn only_a_bare_build_phrase_typed_with_a_plan_to_build_builds_it() {
        let plan = Some(".aura/plans/p_00000001.plan.md");
        let typed = |text: &str| Prompt::typed(text.to_string(), Vec::new());
        for text in [
            "impl",
            "Implement",
            "  implement it.  ",
            "Implement the plan!",
            "implement plan",
            "BUILD",
            "build it!!",
            "build the plan.",
        ] {
            assert!(asks_to_build(&typed(text), plan, false), "{text:?}");
        }
        for text in [
            "implement the parser",
            "build it and commit",
            "implement?",
            "commit",
            "keep going",
            "",
        ] {
            assert!(!asks_to_build(&typed(text), plan, false), "{text:?}");
        }
        assert!(!asks_to_build(&typed("implement"), None, false), "no plan");
        assert!(
            !asks_to_build(&typed("implement"), plan, true),
            "already building it"
        );
        assert!(
            !asks_to_build(&Prompt::notice("implement".to_string()), plan, false),
            "a harness notice"
        );
        assert!(
            !asks_to_build(&Prompt::plain("implement".to_string()), plan, false),
            "written by the harness"
        );
    }

    #[test]
    fn a_plan_is_open_while_a_todo_is_pending_or_in_progress() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(PLANS_DIR)).unwrap();
        let path = ".aura/plans/p_00000001.plan.md";
        let write = |second: &str| {
            let text = format!(
                "---\nname: P\ntodos:\n  - id: a\n    content: A\n    status: completed\n  \
                 - id: b\n    content: B\n    status: {second}\n---\n"
            );
            std::fs::write(dir.path().join(path), text).unwrap();
        };

        assert!(open_plan(dir.path(), None).is_none(), "no plan");
        assert!(open_plan(dir.path(), Some(path)).is_none(), "no plan file");
        assert!(
            open_plan(dir.path(), Some("notes/p.plan.md")).is_none(),
            "not a plan path"
        );
        for closed in ["completed", "cancelled"] {
            write(closed);
            assert!(open_plan(dir.path(), Some(path)).is_none(), "{closed}");
        }
        for open in ["pending", "in_progress"] {
            write(open);
            let plan = open_plan(dir.path(), Some(path));
            assert_eq!(plan.map(|p| p.meta.todos.len()), Some(2), "{open}");
        }
    }

    #[test]
    fn a_list_with_every_item_closed_is_dropped() {
        let item = |status| TodoItem {
            id: "a".into(),
            content: "A".into(),
            status,
            assignee: None,
        };
        let done = vec![item(TodoStatus::Completed), item(TodoStatus::Cancelled)];
        assert!(unfinished_todos(done).is_empty());
        for open in [TodoStatus::Pending, TodoStatus::InProgress] {
            let list = vec![item(TodoStatus::Completed), item(open)];
            assert_eq!(unfinished_todos(list.clone()), list, "{open:?}");
        }
        assert!(unfinished_todos(Vec::new()).is_empty());
    }

    #[test]
    fn a_plan_is_finished_once_it_has_todos_and_all_are_closed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(PLANS_DIR)).unwrap();
        let path = ".aura/plans/p_00000001.plan.md";
        let write = |todos: &str| {
            std::fs::write(dir.path().join(path), format!("---\nname: P\n{todos}---\n")).unwrap();
        };

        assert!(!is_finished_plan(dir.path(), path), "no plan file");
        write("todos: []\n");
        assert!(!is_finished_plan(dir.path(), path), "no todos yet");
        write("todos:\n  - id: a\n    content: A\n    status: in_progress\n");
        assert!(!is_finished_plan(dir.path(), path), "a todo open");
        write("todos:\n  - id: a\n    content: A\n    status: completed\n");
        assert!(is_finished_plan(dir.path(), path));
        assert!(!is_finished_plan(dir.path(), "notes/p.plan.md"));
    }

    #[test]
    fn a_list_with_the_plans_todo_ids_is_its_build_list() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(PLANS_DIR)).unwrap();
        write_plan(dir.path(), "p_00000001.plan.md", "P", 0);
        let path = ".aura/plans/p_00000001.plan.md";
        let item = |id: &str, status| TodoItem {
            id: id.into(),
            content: "anything".into(),
            status,
            assignee: None,
        };

        // Status and content may change while building; the ids decide.
        assert!(is_plan_todo_list(
            dir.path(),
            path,
            &[item("a", TodoStatus::InProgress)]
        ));
        assert!(!is_plan_todo_list(
            dir.path(),
            path,
            &[item("b", TodoStatus::Pending)]
        ));
        assert!(!is_plan_todo_list(dir.path(), path, &[]));
        assert!(!is_plan_todo_list(
            dir.path(),
            ".aura/plans/missing.plan.md",
            &[item("a", TodoStatus::Pending)]
        ));
    }
}
