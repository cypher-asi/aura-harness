//! The channel's TASKS tab, right of QUEUE in the channel's bar.

use super::channels::channels_app;
use super::*;
use crate::events::{PlanView, TodoState, TodoView, UiCommand};

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
fn test_the_tasks_tab_right_of_queue_draws_the_projects_plans() {
    use crate::app::SidePanelTab;
    use crossterm::event::{MouseButton, MouseEventKind};
    let mut app = channel_tasks_app();
    let rows = render_rows(&mut app, 140);
    let bar = line_with(&rows, "QUEUE");
    let queue = char_column(&rows[bar], "QUEUE");
    let after_queue: String = rows[bar].chars().skip(queue).collect();
    let tasks = queue + char_column(&after_queue, "TASKS");
    let at = |n: usize| u16::try_from(n).unwrap();
    let click = MouseEventKind::Down(MouseButton::Left);
    mouse_at(&mut app, click, (at(tasks + 1), at(bar)));
    assert_eq!(app.side_tab(), SidePanelTab::Tasks);

    let rows = render_rows(&mut app, 140);
    assert!(line_with(&rows, "Auth rework") > bar);
    assert!(line_with(&rows, "Write the login handler") > line_with(&rows, "Auth rework"));
    assert!(!rows.join("\n").contains("KERNEL"), "no Kernel panel");
    let side = app.areas.sandbox.expect("the channel's panel");
    let chat = app.areas.chat.expect("the channel's chat");
    assert_eq!(side.y, chat.y, "nothing above it in the column");
}

#[test]
fn test_a_hosted_channel_hands_its_five_tabs_to_the_front_end() {
    use crate::app::{Pane, PaneTab, SidePanelTab};

    let mut app = channel_tasks_app();
    app.set_host_chrome(true);
    render_rows(&mut app, 140);
    let chrome = app.pane_chrome();
    assert!(chrome.iter().all(|bar| bar.pane != Pane::Records));
    let bar = chrome
        .iter()
        .find(|bar| bar.pane == Pane::Side)
        .expect("the channel's bar");
    assert_eq!(
        bar.tabs,
        [
            PaneTab::Side(SidePanelTab::ChannelMembers),
            PaneTab::Side(SidePanelTab::ChannelInfo),
            PaneTab::Side(SidePanelTab::ChannelQueue),
            PaneTab::Side(SidePanelTab::Tasks),
            PaneTab::Side(SidePanelTab::ChannelHistory),
        ]
    );
}
