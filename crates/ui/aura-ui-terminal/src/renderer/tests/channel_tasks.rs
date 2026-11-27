//! The channel's TASKS, drawn in the Record panel's place above its
//! MEMBERS and INFO.

use super::channels::channels_app;
use super::*;
use crate::events::{PlanView, TaskState, TaskView, TodoState, TodoView, UiCommand};

/// `#zode` open, with a plan in its folder whose first todo is done.
fn channel_tasks_app() -> crate::App {
    let todo = |content: &str, state| TodoView {
        content: content.to_string(),
        state,
        assignee: None,
        task: None,
    };
    let mut app = channels_app();
    app.open_channel_by_id("c1");
    app.process_command(UiCommand::SetPlans(vec![PlanView {
        path: ".aura/plans/auth.plan.md".to_string(),
        name: "Auth rework".to_string(),
        todos: vec![
            todo("Add session table", TodoState::Completed),
            todo("Write the login handler", TodoState::Pending),
        ],
    }]));
    app
}

#[test]
fn test_an_open_channel_draws_its_tasks_above_its_members() {
    let mut app = channel_tasks_app();
    let rows = render_rows(&mut app, 140);
    let at = |needle: &str| {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("no {needle:?} in:\n{}", rows.join("\n")))
    };
    let bar = at("TASKS");
    assert!(rows[bar].contains("1 PLAN"), "{}", rows[bar]);
    assert!(at("Auth rework") > bar);
    assert!(at("Write the login handler") > at("Auth rework"));
    assert!(at("MEMBERS") > at("Write the login handler"));
    assert!(!rows.join("\n").contains("KERNEL"), "no Kernel panel");
    let (tasks, members) = (app.areas.records, app.areas.sandbox);
    let (tasks, members) = (tasks.expect("TASKS"), members.expect("MEMBERS"));
    assert_eq!((tasks.x, tasks.width), (members.x, members.width));
    assert!(tasks.bottom() <= members.y, "TASKS on top");

    // While an agent works on a task, the bar counts the running ones.
    app.process_command(UiCommand::SetTasks(vec![TaskView {
        id: "t1".to_string(),
        title: "Write the login handler".to_string(),
        agent_id: "wk".to_string(),
        state: TaskState::Running,
        ..TaskView::default()
    }]));
    let rows = render_rows(&mut app, 140);
    assert!(rows[bar].contains("1 RUNNING"), "{}", rows[bar]);
}

#[test]
fn test_a_hosted_channel_hands_its_tasks_bar_to_the_front_end() {
    use crate::app::{Pane, PaneTab, RecordsTab};

    let mut app = channel_tasks_app();
    app.set_host_chrome(true);
    render_rows(&mut app, 140);
    let bar = app
        .pane_chrome()
        .iter()
        .find(|chrome| chrome.pane == Pane::Records)
        .expect("the TASKS bar");
    assert_eq!(bar.tabs, [PaneTab::Records(RecordsTab::ChannelTasks)]);
    assert_eq!(bar.active, Some(PaneTab::Records(RecordsTab::ChannelTasks)));
    assert_eq!(bar.info.as_deref(), Some("1 PLAN"));
}
