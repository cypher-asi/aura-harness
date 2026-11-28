//! The channel's TASKS, in the Record panel's place while a channel is
//! open: every plan and task in the channel's folder.

use super::channel_member_chat::{app_in_channel_with_events, switch_request};
use super::*;
use crate::events::{PlanView, TaskView, TodoState, TodoView};

fn plan(path: &str, todos: &[&str]) -> PlanView {
    PlanView {
        path: path.to_string(),
        name: path.trim_end_matches(".plan.md").to_string(),
        todos: todos
            .iter()
            .map(|content| TodoView {
                content: (*content).to_string(),
                state: TodoState::Pending,
                assignee: None,
                task: None,
            })
            .collect(),
    }
}

fn task(id: &str, agent: &str) -> TaskView {
    TaskView {
        id: id.to_string(),
        title: id.to_string(),
        agent_id: agent.to_string(),
        ..TaskView::default()
    }
}

/// `#zode`, coordinated by `bb` and open on it, with a plan of two todos
/// and a task by each of `aa`, `bb` and `cc`, an agent of no channel
/// working in the same folder.
fn channel_with_tasks() -> (App, mpsc::Receiver<UiEvent>) {
    let (mut app, mut rx) = app_with_agents();
    app.process_command(UiCommand::SetChannels(vec![zode("bb")]));
    app.open_channel_by_id("c1");
    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    while rx.try_recv().is_ok() {}
    app.process_command(UiCommand::SetPlans(vec![plan(
        "auth.plan.md",
        &["Add the table", "Write the handler"],
    )]));
    app.process_command(UiCommand::SetTasks(vec![
        task("ta", "aa"),
        task("tb", "bb"),
        task("tc", "cc"),
    ]));
    (app, rx)
}

fn listed_tasks(app: &App) -> Vec<&str> {
    let list = app.task_list();
    list.scoped_tasks().map(|task| task.id.as_str()).collect()
}

fn plan_names(app: &App) -> Vec<&str> {
    let list = app.task_list();
    list.plans().iter().map(|plan| plan.name.as_str()).collect()
}

#[test]
fn test_the_channels_tasks_list_every_agents_work_in_its_folder() {
    let (mut app, _rx) = channel_with_tasks();
    assert_eq!(app.side_scope(), SideScope::Agent, "the user's scope");
    assert_eq!(listed_tasks(&app), ["ta", "tb", "tc"]);
    assert_eq!(app.task_list().hidden(), 0);
    assert_eq!(app.task_list().hidden_plans(), 0);

    // Back among the agents, TASKS lists the side panel's scope again.
    app.focus = PanelFocus::Swarm;
    app.handle_key(key(KeyCode::Right));
    assert!(!app.channel_open());
    assert_eq!(app.task_list().hidden(), 2);
}

#[test]
fn test_the_channels_tasks_take_the_keys_and_the_wheel() {
    let (mut app, _rx) = channel_with_tasks();
    app.focus = PanelFocus::Records;
    assert!(app.tasks_focused());
    assert_eq!(app.task_list().rows().len(), 3, "the plan and its todos");

    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.task_list().selected(), 1);
    app.handle_key(key(KeyCode::End));
    assert_eq!(app.task_list().selected(), 2);
    app.handle_key(key(KeyCode::Home));
    assert_eq!(app.task_list().selected(), 0);
    app.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(app.task_list().rows().len(), 1, "Space folds the plan");
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.task_list().rows().len(), 3, "Right opens it");
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    assert_eq!(app.records_tab(), RecordsTab::ChannelTasks, "its only tab");
    assert!(app.input().is_empty(), "no key was typed");

    app.scroll_panel_down(PanelFocus::Records, 2);
    assert_eq!(app.task_list().selected(), 2);
    app.scroll_panel_up(PanelFocus::Records, 1);
    assert_eq!(app.task_list().selected(), 1);

    app.focus = PanelFocus::Sandbox;
    assert!(!app.tasks_focused(), "MEMBERS has the focus");
}

#[test]
fn test_clicking_and_hovering_a_row_of_the_channels_tasks() {
    let (mut app, _rx) = channel_with_tasks();
    let screen = drawn_text(&mut app);
    assert!(screen.contains("TASKS"), "{screen}");
    let rows = app.areas.task_rows.expect("the tree is drawn on top");
    let todo = (rows.x + 8, rows.y + 2);

    app.handle_mouse(mouse(MouseEventKind::Moved, todo.0, todo.1));
    assert_eq!(app.hover(), Some(HoverTarget::TaskRow(2)));
    click_at(&mut app, todo);
    assert_eq!(app.focus(), PanelFocus::Records);
    assert_eq!(app.task_list().selected(), 2);

    // The plan's fold glyph folds it.
    let fold = rows.x + u16::try_from(crate::components::ROW_INSET).unwrap_or(0);
    click_at(&mut app, (fold, rows.y));
    assert_eq!(app.task_list().rows().len(), 1);
}

#[test]
fn test_enter_on_a_plan_opens_it_over_the_channels_chat() {
    let (mut app, _rx) = channel_with_tasks();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("auth.plan.md"),
        "# Auth\n\n- [ ] Add the table\n- [ ] Write the handler\n",
    )
    .unwrap();
    app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
    app.focus = PanelFocus::Records;
    app.handle_key(key(KeyCode::Enter));
    assert!(
        matches!(app.detail_view(), Some(DetailView::Plan(_))),
        "{:?}",
        app.detail_view()
    );
}

#[test]
fn test_enter_on_a_members_task_opens_its_chat_in_the_channel() {
    let (_dir, mut app, mut rx) = app_in_channel_with_events("co");
    let spawned = |id: &str, agent: &str| TaskView {
        parent: Some("rq".to_string()),
        ..task(id, agent)
    };
    app.process_command(UiCommand::SetTasks(vec![
        task("rq", "co"),
        spawned("tr", "re"),
        spawned("tx", "xx"),
    ]));
    app.focus = PanelFocus::Records;
    select_task(&mut app, "tr");
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(switch_request(&mut rx).as_deref(), Some("re"));
    assert!(app.channel_open());
    assert_eq!(app.member_chat().map(|m| m.id.as_str()), Some("re"));
    assert_eq!(app.selected_channel_member(), 1, "its MEMBERS row too");

    // An agent of no channel is the agent app's to show.
    app.focus = PanelFocus::Records;
    select_task(&mut app, "tx");
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(switch_request(&mut rx).as_deref(), Some("xx"));
}

/// Highlight the row of task `id`.
fn select_task(app: &mut App, id: &str) {
    let rows = app.task_list().rows().len();
    let found = (0..rows).any(|row| {
        app.task_list.select(row);
        app.task_list()
            .selected_task()
            .is_some_and(|task| task.id == id)
    });
    assert!(found, "no row for {id}");
}

#[test]
fn test_a_kernel_record_jump_in_a_channel_only_says_the_panel_is_away() {
    let (mut app, _rx) = channel_with_tasks();
    app.process_command(UiCommand::SetRecords(vec![
        crate::app::detail_tests::record(1),
    ]));
    app.focus = PanelFocus::Chat;
    app.show_turn_records(0);
    assert_eq!(app.records_tab(), RecordsTab::ChannelTasks);
    assert_eq!(app.focus(), PanelFocus::Chat);
    assert!(app
        .notification()
        .is_some_and(|(text, _)| text.contains("PROJECTS")));
}

#[test]
fn test_kernel_panel_commands_leave_the_channels_tasks_and_layout_alone() {
    let (_dir, mut app) = app_in_channel("co");
    let before = (
        app.panels.record_visible,
        app.panels.side_expanded,
        app.panels.sandbox_visible,
    );
    app.show_members();
    app.show_policies();
    app.show_inbox();
    assert_eq!(
        (
            app.panels.record_visible,
            app.panels.side_expanded,
            app.panels.sandbox_visible,
        ),
        before
    );
    assert_eq!(app.records_tab(), RecordsTab::ChannelTasks);
    assert_ne!(app.focus(), PanelFocus::Records);
}

#[test]
fn test_the_hamburger_hides_and_shows_both_of_the_channels_panels() {
    let (mut app, _rx) = channel_with_tasks();
    app.focus = PanelFocus::Records;
    app.toggle_side_column();
    assert!(!app.record_panel_visible());
    assert!(!app.sandbox_panel_visible());
    assert_eq!(app.focus(), PanelFocus::Chat, "off the hidden panel");

    app.toggle_side_column();
    assert!(app.record_panel_visible());
    assert!(app.sandbox_panel_visible());

    // The agent's own flags hide neither.
    app.panels.record_visible = false;
    app.panels.sandbox_visible = true;
    app.panels.side_expanded = true;
    assert!(
        app.record_panel_visible(),
        "not under a maximized side panel"
    );
}

#[test]
fn test_a_plan_another_agent_writes_or_moves_on_shows_at_once() {
    let (mut app, _rx) = channel_with_tasks();
    let upsert = |plan: PlanView| UiCommand::ForAgent {
        agent_id: "bb".to_string(),
        command: Box::new(UiCommand::UpsertPlan(plan)),
    };
    app.process_command(upsert(plan("parser.plan.md", &["Split the lexer"])));
    assert_eq!(plan_names(&app), ["parser", "auth"], "a new plan on top");

    let mut moved_on = plan("auth.plan.md", &["Add the table", "Write the handler"]);
    moved_on.todos[0].state = TodoState::Completed;
    app.process_command(upsert(moved_on));
    assert_eq!(plan_names(&app), ["parser", "auth"], "a known one stays");
    let list = app.task_list();
    assert_eq!(list.plans()[1].progress(), (1, 2));
    assert!(
        list.plans().iter().all(|plan| !list.is_current(plan)),
        "none is marked as the one being built"
    );

    // Only the agent on screen's folder is listed.
    app.process_command(UiCommand::ForAgent {
        agent_id: "aa".to_string(),
        command: Box::new(UiCommand::UpsertPlan(plan("other.plan.md", &[]))),
    });
    assert_eq!(plan_names(&app).len(), 2);
}

#[test]
fn test_the_tasks_tab_is_listed_and_reached_only_in_a_channel() {
    assert!(!RecordsTab::tabs(false).contains(&RecordsTab::ChannelTasks));
    for &tab in RecordsTab::tabs(false) {
        assert!(!tab.next(false).is_channel_tab(), "{tab:?}");
        assert!(!tab.prev(false).is_channel_tab(), "{tab:?}");
    }
    assert_eq!(RecordsTab::Kernel.next(true), RecordsTab::ChannelTasks);
    assert_eq!(RecordsTab::Inbox.prev(true), RecordsTab::ChannelTasks);

    let (mut app, _rx) = app_with_agents();
    app.show_records_tab(RecordsTab::ChannelTasks);
    app.select_pane_tab(PaneTab::Records(RecordsTab::ChannelTasks));
    assert_eq!(app.records_tab(), RecordsTab::Kernel, "no channel is open");
}
