
use super::*;
use crate::app::tasks::TaskRow;
use crate::events::{
    AgentSummary, PlanView, ProcessKind, TaskView, TodoState, TodoView, TraceNode, UiCommand,
};
use crossterm::event::{KeyCode, KeyEvent};

fn agent(id: &str, project: &str, depth: usize) -> AgentSummary {
    AgentSummary {
        id: id.into(),
        name: id.to_uppercase(),
        project_id: Some(project.into()),
        depth,
        ..AgentSummary::default()
    }
}

fn process(id: &str, owner: &str) -> ProcessSummary {
    ProcessSummary {
        id: id.into(),
        kind: ProcessKind::Command,
        label: id.into(),
        owner_agent_id: owner.into(),
        jump_agent_id: owner.into(),
        ..ProcessSummary::default()
    }
}

fn task(id: &str, agent: &str, parent: Option<&str>, plan: Option<&str>) -> TaskView {
    TaskView {
        id: id.into(),
        title: id.into(),
        agent_id: agent.into(),
        parent: parent.map(str::to_string),
        plan: plan.map(str::to_string),
        ..TaskView::default()
    }
}

fn plan(path: &str, todo_task: Option<&str>) -> PlanView {
    PlanView {
        path: path.into(),
        name: path.into(),
        todos: vec![TodoView {
            content: "step".into(),
            state: TodoState::Pending,
            assignee: None,
            task: todo_task.map(str::to_string),
        }],
    }
}

fn step(agent: &str, id: &str) -> UiCommand {
    UiCommand::ForAgent {
        agent_id: agent.into(),
        command: Box::new(UiCommand::GraphUpsert {
            run_id: format!("run-{agent}"),
            nodes: vec![TraceNode {
                id: id.into(),
                kind: "tool_call".into(),
                ..TraceNode::default()
            }],
        }),
    }
}

/// Project `site` with `a` (which spawned `ac`) and `b`; project
/// `notes` with `n`.
fn app() -> App {
    let mut app = App::new();
    app.process_command(UiCommand::SetAgents(vec![
        agent("a", "site", 0),
        agent("ac", "site", 1),
        agent("b", "site", 0),
        agent("n", "notes", 0),
    ]));
    app.process_command(UiCommand::SetActiveAgent("a".into()));
    app.process_command(UiCommand::SetProcesses(vec![
        process("pa", "a"),
        process("pac", "ac"),
        process("pb", "b"),
        process("pn", "n"),
        process("none", ""),
    ]));
    app.process_command(UiCommand::SetTasks(vec![
        task("ta", "a", None, Some("plan-a")),
        task("tac", "ac", Some("ta"), None),
        task("tb", "b", None, Some("plan-b")),
    ]));
    app.process_command(UiCommand::SetPlans(vec![
        plan("plan-a", None),
        plan("plan-b", None),
        plan("plan-c", Some("tac")),
    ]));
    for (agent, id) in [("a", "sa"), ("b", "sb"), ("ac", "sac"), ("n", "sn")] {
        app.process_command(step(agent, id));
    }
    app
}

fn processes(app: &App) -> Vec<&str> {
    app.processes().iter().map(|p| p.id.as_str()).collect()
}

fn steps(app: &App) -> Vec<&str> {
    app.trace_feed()
        .entries()
        .map(|e| e.node.id.as_str())
        .collect()
}

fn tasks(app: &App) -> Vec<&str> {
    let list = app.task_list();
    list.scoped_tasks().map(|t| t.id.as_str()).collect()
}

fn plans(app: &App) -> Vec<&str> {
    let list = app.task_list();
    list.rows()
        .iter()
        .filter_map(|row| match *row {
            crate::app::tasks::TaskRow::Plan {
                plan: Some(plan), ..
            } => Some(list.plans()[plan].path.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_lists_show_the_active_agent_and_its_spawns() {
    let app = app();
    assert_eq!(app.side_scope(), SideScope::Agent);
    assert_eq!(processes(&app), ["pa", "pac", "none"]);
    assert_eq!(steps(&app), ["sa", "sac"]);
    assert_eq!(tasks(&app), ["ta", "tac"]);
    assert_eq!(plans(&app), ["plan-a", "plan-c"]);
    assert_eq!(app.task_list().hidden(), 1);
    assert_eq!(app.task_list().hidden_plans(), 1);
}

#[test]
fn all_shows_the_whole_project_and_no_other() {
    let mut app = app();
    app.toggle_side_scope();
    assert_eq!(app.side_scope(), SideScope::Project);
    assert_eq!(processes(&app), ["pa", "pac", "pb", "none"]);
    assert_eq!(steps(&app), ["sa", "sb", "sac"]);
    assert_eq!(tasks(&app), ["ta", "tac", "tb"]);
    assert_eq!(plans(&app), ["plan-a", "plan-b", "plan-c"]);

    app.toggle_side_scope();
    assert_eq!(processes(&app), ["pa", "pac", "none"]);
}

#[test]
fn the_lists_follow_the_active_agent() {
    let mut app = app();
    app.process_command(UiCommand::SetActiveAgent("b".into()));
    assert_eq!(processes(&app), ["pb", "none"]);
    assert_eq!(steps(&app), ["sb"]);
    assert_eq!(tasks(&app), ["tb"]);
    assert_eq!(plans(&app), ["plan-b"]);

    app.process_command(UiCommand::SetActiveAgent("ac".into()));
    assert_eq!(processes(&app), ["pac", "none"]);
    assert_eq!(steps(&app), ["sac"]);
    assert_eq!(tasks(&app), ["tac"]);

    app.toggle_side_scope();
    app.process_command(UiCommand::SetActiveAgent("n".into()));
    assert_eq!(processes(&app), ["pn", "none"]);
    assert_eq!(steps(&app), ["sn"]);
}

#[test]
fn a_new_spawn_joins_its_parents_lists() {
    let mut app = app();
    app.process_command(UiCommand::SetAgents(vec![
        agent("a", "site", 0),
        agent("ac", "site", 1),
        agent("b", "site", 0),
        agent("bc", "site", 1),
        agent("n", "notes", 0),
    ]));
    app.process_command(UiCommand::SetActiveAgent("b".into()));
    app.process_command(step("bc", "sbc"));
    assert_eq!(steps(&app), ["sb", "sbc"]);
}

#[test]
fn the_highlight_stays_on_its_row_across_the_switch() {
    let mut app = app();
    app.processes.selected = 1;
    app.toggle_side_scope();
    assert_eq!(
        app.selected_process_data().map(|p| p.id.as_str()),
        Some("pac")
    );
    app.toggle_side_scope();
    assert_eq!(
        app.selected_process_data().map(|p| p.id.as_str()),
        Some("pac")
    );
}

#[test]
fn an_agent_comes_back_with_its_lists_where_it_left_them() {
    let mut app = app();
    app.processes.selected = 1;
    app.processes.scroll = 1;
    app.trace_feed.select(0);
    app.trace_scroll = 1;
    app.task_list.select(1);
    app.tasks_scroll = 1;
    let task = highlighted(&app);

    app.process_command(UiCommand::SetActiveAgent("b".into()));
    assert_eq!(processes(&app), ["pb", "none"]);
    app.processes.selected = 0;
    app.processes.scroll = 0;
    app.trace_scroll = 0;
    app.tasks_scroll = 0;

    app.process_command(UiCommand::SetActiveAgent("a".into()));
    assert_eq!(
        app.selected_process_data().map(|p| p.id.as_str()),
        Some("pac")
    );
    assert_eq!(
        app.trace_feed()
            .selected_entry()
            .map(|e| e.node.id.as_str()),
        Some("sa")
    );
    assert_eq!(highlighted(&app), task);
    assert_eq!(
        (app.processes.scroll, app.trace_scroll, app.tasks_scroll),
        (1, 1, 1)
    );
}

#[test]
fn a_trace_left_on_its_newest_step_follows_new_ones() {
    let mut app = app();
    app.process_command(UiCommand::SetActiveAgent("b".into()));
    app.process_command(step("a", "sa2"));
    app.process_command(UiCommand::SetActiveAgent("a".into()));
    assert_eq!(
        app.trace_feed()
            .selected_entry()
            .map(|e| e.node.id.as_str()),
        Some("sa2")
    );
}

/// The TASKS tab's rows as text, each indented two spaces per level:
/// a plan header by path, a todo as `#` and its plan's path, an item
/// by its content, a task as `@` and its id.
fn rows(app: &App) -> Vec<String> {
    let list = app.task_list();
    list.rows()
        .iter()
        .map(|&row| {
            let text = match row {
                TaskRow::Plan { plan, .. } => {
                    plan.map_or_else(|| "No Plan".into(), |plan| list.plans()[plan].path.clone())
                }
                TaskRow::Todo { plan, .. } => format!("#{}", list.plans()[plan].path),
                TaskRow::Item { task, item, .. } => list.tasks()[task].items[item].content.clone(),
                TaskRow::Round { first, .. } => format!("~{}", list.tasks()[first].id),
                TaskRow::Task { task, .. } => format!("@{}", list.tasks()[task].id),
            };
            format!("{}{text}", "  ".repeat(row.depth()))
        })
        .collect()
}

fn highlighted(app: &App) -> Option<String> {
    rows(app).get(app.task_list().selected()).cloned()
}

/// `tx` of `a` is a part of `tb` of `b` and does `plan-c`'s todo;
/// `tn` of `b` works on no plan and has an item.
fn app_with_cross_agent_tasks() -> App {
    let mut app = app();
    let mut tn = task("tn", "b", None, None);
    tn.items = vec![TodoView {
        content: "tn's".into(),
        state: TodoState::Pending,
        assignee: None,
        task: None,
    }];
    app.process_command(UiCommand::SetTasks(vec![
        tn,
        task("tx", "a", Some("tb"), None),
        task("tb", "b", None, Some("plan-b")),
    ]));
    app.process_command(UiCommand::SetPlans(vec![
        plan("plan-b", None),
        plan("plan-c", Some("tx")),
    ]));
    app
}

#[test]
fn the_scope_moves_tasks_between_plans_and_shows_no_plan_when_needed() {
    let mut app = app_with_cross_agent_tasks();
    assert_eq!(
        rows(&app),
        ["plan-c", "#plan-c", "  @tx"],
        "without tb, tx is a task under the todo of plan-c it does"
    );

    app.toggle_side_scope();
    assert_eq!(
        rows(&app),
        ["plan-b", "#plan-b", "@tx", "plan-c", "#plan-c", "No Plan", "tn's"],
        "tx joins tb's plan, and tn's item needs No Plan"
    );

    app.toggle_side_scope();
    assert_eq!(
        rows(&app),
        ["plan-c", "#plan-c", "  @tx"],
        "No Plan goes again"
    );
}

#[test]
fn the_highlight_stays_on_a_plan_or_todo_across_the_scope_switch() {
    let mut app = app_with_cross_agent_tasks();
    app.task_list.select(0);
    app.toggle_side_scope();
    assert_eq!(app.task_list().selected(), 3);
    assert_eq!(highlighted(&app).as_deref(), Some("plan-c"));
    app.toggle_side_scope();
    assert_eq!(highlighted(&app).as_deref(), Some("plan-c"));

    app.task_list.select(1);
    app.toggle_side_scope();
    assert_eq!(app.task_list().selected(), 4);
    assert_eq!(highlighted(&app).as_deref(), Some("#plan-c"));
    app.toggle_side_scope();
    assert_eq!(app.task_list().selected(), 1);
}

#[test]
fn dropping_the_current_plan_hides_it_and_clamps_the_highlight() {
    let mut app = app();
    app.process_command(UiCommand::SetTasks(vec![task(
        "ta",
        "a",
        None,
        Some("plan-a"),
    )]));
    app.process_command(UiCommand::SetPlans(vec![plan("plan-a", None)]));
    let mut current = plan("plan-z", None);
    current.todos = vec![current.todos[0].clone(); 4];
    app.process_command(UiCommand::SetPlan(Some(current)));
    assert_eq!(rows(&app).len(), 7, "plan-z and its 4 todos, then plan-a");
    app.task_list.select(4);
    assert_eq!(highlighted(&app).as_deref(), Some("#plan-z"));

    app.process_command(UiCommand::SetPlan(None));
    assert_eq!(rows(&app), ["plan-a", "#plan-a"]);
    assert_eq!(app.task_list().selected(), 1);
    assert_eq!(app.task_list().hidden_plans(), 1);
}

#[test]
fn t_on_a_todo_whose_task_the_scope_hides_points_at_the_button() {
    let mut app = app();
    app.process_command(UiCommand::SetPlans(vec![plan("plan-a", Some("tb"))]));
    app.focus = crate::app::PanelFocus::Sandbox;
    app.show_side_tab(crate::app::SidePanelTab::Tasks);
    app.task_list.select(1);
    assert_eq!(highlighted(&app).as_deref(), Some("#plan-a"));

    app.handle_key(KeyEvent::from(KeyCode::Char('t')));
    assert_eq!(
        app.notification().map(|(text, _)| text.as_str()),
        Some("Another agent is doing this todo; the * button lists it")
    );
    assert_eq!(app.task_list().selected(), 1, "the highlight stays put");
}

#[test]
fn processes_of_no_agent_are_always_shown() {
    let none = process("x", "");
    let agents = HashSet::from(["a".to_string()]);
    assert!(process_in_scope(&none, &agents));
    let mut spawned = process("y", "child");
    assert!(!process_in_scope(&spawned, &agents));
    spawned.parent_chain = vec!["a".into()];
    assert!(process_in_scope(&spawned, &agents));
}
