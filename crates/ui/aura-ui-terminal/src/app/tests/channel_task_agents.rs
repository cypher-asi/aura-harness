//! A task or to-do row in a channel opens the agent it was given to: in
//! TASKS by a click or Enter, in the to-do drawer by a click. A row given
//! to no agent opens nothing.

use super::channel_member_chat::{app_in_channel_with_events, switch_request};
use super::*;
use crate::app::tasks::TaskRow;
use crate::events::{TaskView, TodoState, TodoView};

fn item(content: &str, assignee: Option<&str>) -> TodoView {
    TodoView {
        content: content.to_string(),
        state: TodoState::Pending,
        assignee: assignee.map(str::to_string),
        task: None,
    }
}

/// The coordinator's request `rq`, whose list gives an item to `RE36`
/// (member `re`) and leaves one to no agent, and a task of no agent.
fn channel_with_items() -> (tempfile::TempDir, App, mpsc::Receiver<UiEvent>) {
    let (dir, mut app, mut rx) = app_in_channel_with_events("co");
    let request = TaskView {
        id: "rq".to_string(),
        title: "Ship it".to_string(),
        agent_id: "co".to_string(),
        agent_name: "Lead".to_string(),
        items: vec![
            item("Review the parser", Some("RE36")),
            item("Tidy the docs", None),
        ],
        ..TaskView::default()
    };
    let orphan = TaskView {
        id: "tx".to_string(),
        title: "Nobody's".to_string(),
        parent: Some("rq".to_string()),
        ..TaskView::default()
    };
    app.process_command(UiCommand::SetTasks(vec![request, orphan]));
    app.show_side_tab(SidePanelTab::Tasks);
    while rx.try_recv().is_ok() {}
    (dir, app, rx)
}

/// Highlight the row of item `content`, returning its index.
fn select_item(app: &mut App, content: &str) -> usize {
    let rows = app.task_list().rows().to_vec();
    let at = rows
        .iter()
        .position(|&row| match row {
            TaskRow::Item { task, item, .. } => app
                .task_list()
                .item(task, item)
                .is_some_and(|item| item.content == content),
            _ => false,
        })
        .unwrap_or_else(|| panic!("no row for {content}: {rows:?}"));
    app.task_list.select(at);
    at
}

#[test]
fn test_enter_on_an_item_opens_the_member_it_was_given_to() {
    let (_dir, mut app, mut rx) = channel_with_items();
    app.focus = PanelFocus::Sandbox;
    select_item(&mut app, "Review the parser");
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(switch_request(&mut rx).as_deref(), Some("re"));
    assert_eq!(app.member_chat().map(|m| m.id.as_str()), Some("re"));
}

#[test]
fn test_a_click_on_an_item_opens_the_member_it_was_given_to() {
    let (_dir, mut app, mut rx) = channel_with_items();
    drawn_text(&mut app);
    let at = select_item(&mut app, "Review the parser");
    let rows = app.areas.task_rows.expect("the tree is drawn");
    let row = rows.y + u16::try_from(at - app.areas.tasks_scroll).unwrap();
    click_at(&mut app, (rows.x + 8, row));
    assert_eq!(switch_request(&mut rx).as_deref(), Some("re"));
    assert_eq!(app.member_chat().map(|m| m.id.as_str()), Some("re"));
}

#[test]
fn test_rows_given_to_no_agent_open_nothing() {
    let (_dir, mut app, mut rx) = channel_with_items();
    app.focus = PanelFocus::Sandbox;
    select_item(&mut app, "Tidy the docs");
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(switch_request(&mut rx), None, "not the request's agent");
    assert!(app.member_chat().is_none());

    let rows = app.task_list().rows().len();
    let found = (0..rows).any(|row| {
        app.task_list.select(row);
        app.task_list()
            .selected_task()
            .is_some_and(|task| task.id == "tx")
    });
    assert!(found, "the task of no agent has a row");
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(switch_request(&mut rx), None);
    assert!(app.member_chat().is_none());
}

/// Press the left button on the first cell of `needle` on screen.
fn click_text(app: &mut App, needle: &str) {
    let screen = drawn_text(app);
    let (row, line) = screen
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains(needle))
        .unwrap_or_else(|| panic!("no {needle:?} in\n{screen}"));
    let column = line.find(needle).map_or(0, |x| line[..x].chars().count());
    click_at(
        app,
        (
            u16::try_from(column).unwrap(),
            u16::try_from(row).unwrap(),
        ),
    );
}

#[test]
fn test_a_click_on_a_drawer_item_opens_the_agent_it_was_given_to() {
    let (_dir, mut app, mut rx) = app_in_channel_with_events("co");
    app.process_command(UiCommand::SetTodos(vec![
        item("Review the parser", Some("RE36")),
        item("Tidy the docs", None),
    ]));
    while rx.try_recv().is_ok() {}
    click_text(&mut app, "Tidy the docs");
    assert_eq!(switch_request(&mut rx), None, "given to no agent");
    assert!(app.member_chat().is_none());

    click_text(&mut app, "Review the parser");
    assert_eq!(switch_request(&mut rx).as_deref(), Some("re"));
    assert_eq!(app.member_chat().map(|m| m.id.as_str()), Some("re"));
}

#[test]
fn test_enter_on_a_plans_todo_opens_the_agent_of_its_task() {
    use crate::events::PlanView;
    let (_dir, mut app, mut rx) = app_in_channel_with_events("co");
    app.process_command(UiCommand::SetPlans(vec![PlanView {
        path: "auth.plan.md".to_string(),
        name: "auth".to_string(),
        todos: vec![TodoView {
            task: Some("tr".to_string()),
            ..item("Write the handler", None)
        }],
        changed_ms: None,
    }]));
    app.process_command(UiCommand::SetTasks(vec![TaskView {
        id: "tr".to_string(),
        title: "Write the handler".to_string(),
        agent_id: "re".to_string(),
        plan: Some("auth.plan.md".to_string()),
        ..TaskView::default()
    }]));
    app.show_side_tab(SidePanelTab::Tasks);
    app.focus = PanelFocus::Sandbox;
    while rx.try_recv().is_ok() {}
    let rows = app.task_list().rows().to_vec();
    let todo = rows
        .iter()
        .position(|row| matches!(row, TaskRow::Todo { .. }))
        .unwrap_or_else(|| panic!("no todo row: {rows:?}"));
    app.task_list.select(todo);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(switch_request(&mut rx).as_deref(), Some("re"));
}
