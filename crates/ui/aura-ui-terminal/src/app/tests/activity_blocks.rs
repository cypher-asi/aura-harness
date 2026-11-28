use super::*;

#[test]
fn test_thinking_streams_into_one_block() {
    let mut app = App::new();
    app.process_command(UiCommand::StartThinking);
    app.process_command(UiCommand::AppendThinking("Let me ".to_string()));
    app.process_command(UiCommand::AppendThinking("look.".to_string()));
    assert!(app.has_live_activity());

    app.process_command(UiCommand::FinishThinking);
    let [Activity::Thinking(step)] = activities(&app)[..] else {
        panic!("expected one thinking block");
    };
    assert_eq!(step.text, "Let me look.");
    assert!(!step.running);
    assert!(step.duration_ms.is_some());
}

#[test]
fn test_tool_block_updates_in_place() {
    let mut app = App::new();
    app.process_command(tool_data("t1", "read_file"));
    app.process_command(tool_data("t2", "run_command"));
    app.process_command(UiCommand::UpdateToolArgs {
        id: "t1".to_string(),
        args: r#"{"path":"a.rs"}"#.to_string(),
    });
    app.process_command(UiCommand::ToolRetrying {
        id: "t2".to_string(),
        attempt: 2,
        max_attempts: 3,
    });
    app.process_command(UiCommand::CompleteTool {
        id: "t1".to_string(),
        result: "fn main() {}".to_string(),
        success: true,
    });

    let [Activity::Tool(first), Activity::Tool(second)] = activities(&app)[..] else {
        panic!("expected two tool blocks");
    };
    assert_eq!(first.args, r#"{"path":"a.rs"}"#);
    assert_eq!(first.status, ToolStatus::Success);
    assert_eq!(first.result.as_deref(), Some("fn main() {}"));
    assert_eq!(second.status, ToolStatus::Running);
    assert_eq!(second.retry, Some((2, 3)));
    assert_eq!(app.messages().len(), 2);
}

#[test]
fn test_complete_ends_running_activity() {
    let mut app = App::new();
    app.process_command(UiCommand::StartThinking);
    app.process_command(tool_data("t1", "run_command"));
    app.process_command(UiCommand::Complete);

    assert!(!app.has_live_activity());
    let [_, Activity::Tool(tool)] = activities(&app)[..] else {
        panic!("expected thinking then a tool");
    };
    assert_eq!(tool.status, ToolStatus::Error);
}

#[test]
fn test_ctrl_o_toggles_every_block_and_new_ones_follow() {
    let mut app = App::new();
    app.process_command(tool_data("t1", "read_file"));
    assert!(!app.messages()[0].is_expanded());

    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    assert!(app.messages()[0].is_expanded());
    app.process_command(UiCommand::StartThinking);
    assert!(app.messages()[1].is_expanded());

    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    assert!(app.messages().iter().all(|m| !m.is_expanded()));
    assert!(app.input().is_empty());
}

#[test]
fn test_clicking_a_block_header_toggles_only_it() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

    let mut app = App::new();
    app.process_command(tool_data("t1", "read_file"));
    app.process_command(tool_data("t2", "read_file"));
    let ids: Vec<u64> = app.messages().iter().map(|m| m.version().0).collect();
    app.areas.chat = Some(Rect::new(0, 0, 60, 20));
    app.set_chat_text_area(Rect::new(2, 1, 56, 18));
    app.set_chat_layout(step_rows(&ids), 0);

    let click = |app: &mut App, kind| {
        app.handle_mouse(MouseEvent {
            kind,
            column: 10,
            row: 2,
            modifiers: KeyModifiers::empty(),
        });
    };
    click(&mut app, MouseEventKind::Down(MouseButton::Left));
    click(&mut app, MouseEventKind::Up(MouseButton::Left));

    assert!(!app.messages()[0].is_expanded());
    assert!(app.messages()[1].is_expanded());
}

#[test]
fn test_clicking_a_group_summary_flips_what_it_shows() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

    let mut app = App::new();
    app.areas.chat = Some(Rect::new(0, 0, 60, 20));
    app.set_chat_text_area(Rect::new(2, 1, 56, 18));
    let click = |app: &mut App| {
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.handle_mouse(MouseEvent {
                kind,
                column: 10,
                row: 1,
                modifiers: KeyModifiers::empty(),
            });
        }
    };
    let group = |open| ToggleRow {
        line: 0,
        target: ToggleTarget::Group(7),
        open,
    };

    app.set_chat_layout(vec![group(false)], 0);
    click(&mut app);
    assert_eq!(app.chat.group_open.get(&7), Some(&true));

    app.set_chat_layout(vec![group(true)], 0);
    click(&mut app);
    assert_eq!(app.chat.group_open.get(&7), Some(&false));

    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    assert!(app.chat.group_open.is_empty());
}

#[test]
fn test_the_todo_drawer_folds_and_an_emptied_list_opens_it_again() {
    use crate::events::{TodoState, TodoView};

    let mut app = App::new();
    assert!(!app.todo_drawer_folded());
    app.toggle_todo_drawer();
    assert!(app.todo_drawer_folded());
    app.toggle_todo_drawer();
    assert!(!app.todo_drawer_folded());

    // A new list keeps the fold; an emptied one opens the next list.
    app.toggle_todo_drawer();
    app.process_command(UiCommand::SetTodos(vec![TodoView {
        content: "next".to_string(),
        state: TodoState::Pending,
        assignee: None,
        task: None,
    }]));
    assert!(app.todo_drawer_folded());
    app.process_command(UiCommand::SetTodos(Vec::new()));
    assert!(!app.todo_drawer_folded());
}

#[test]
fn test_clicking_a_long_tool_result_opens_it_in_the_file_view() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

    let mut app = App::new();
    for (id, output) in [("t1", "a\n".repeat(30)), ("t2", "short".to_string())] {
        app.process_command(UiCommand::ShowTool(crate::events::ToolData {
            id: id.to_string(),
            name: "run_command".to_string(),
            args: r#"{"program":"cargo","args":["test"]}"#.to_string(),
        }));
        app.process_command(UiCommand::CompleteTool {
            id: id.to_string(),
            result: output,
            success: true,
        });
    }
    let ids: Vec<u64> = app.messages().iter().map(|m| m.version().0).collect();
    app.areas.chat = Some(Rect::new(0, 0, 60, 20));
    app.set_chat_text_area(Rect::new(2, 1, 56, 18));
    app.set_chat_layout(step_rows(&ids), 0);
    let click = |app: &mut App, row| {
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.handle_mouse(MouseEvent {
                kind,
                column: 10,
                row,
                modifiers: KeyModifiers::empty(),
            });
        }
    };

    click(&mut app, 2);
    assert!(app.detail_view().is_none(), "short output expands inline");
    assert!(app.messages()[1].is_expanded());

    click(&mut app, 1);
    let Some(DetailView::File(view)) = app.detail_view() else {
        panic!("long output opens in the file view");
    };
    assert_eq!(view.extension(), "log");
    assert!(!app.messages()[0].is_expanded());
}

const FULL_ACCESS: &str = "allowed by user_default.full_access";

fn list_files_policy(id: &str) -> UiCommand {
    UiCommand::ToolPolicy {
        id: id.to_string(),
        tool: "list_files".to_string(),
        decision: FULL_ACCESS.to_string(),
    }
}

/// A cloud turn's copied record can bring a call's decision before its
/// stream brings the call; a `policy:` line for it would split the turn's
/// run of calls.
#[test]
fn test_a_policy_ahead_of_its_call_waits_for_it() {
    let mut app = App::new();
    app.process_command(UiCommand::TurnStarted { prompt: None });
    app.process_command(list_files_policy("t1"));
    app.process_command(tool_data("t1", "list_files"));

    let [Activity::Tool(tool)] = activities(&app)[..] else {
        panic!("expected one tool block");
    };
    assert_eq!(tool.policy.as_deref(), Some(FULL_ACCESS));
    assert_eq!(app.messages().len(), 1, "and no policy line");
}

#[test]
fn test_a_policy_whose_call_never_comes_shows_as_the_turn_ends() {
    let mut app = App::new();
    app.process_command(UiCommand::TurnStarted { prompt: None });
    app.process_command(list_files_policy("t1"));
    assert!(app.messages().is_empty(), "the call may still come");

    app.process_command(UiCommand::Complete);
    let lines: Vec<&str> = app.messages().iter().map(Message::content).collect();
    assert_eq!(lines, [format!("policy: list_files {FULL_ACCESS}")]);
}

#[test]
fn test_a_policy_ahead_of_its_call_waits_in_its_agent_s_chat() {
    let mut app = App::new();
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app.process_command(UiCommand::TurnStarted { prompt: None });
    app.process_command(for_agent("bb", UiCommand::TurnStarted { prompt: None }));
    app.process_command(for_agent("bb", list_files_policy("t1")));
    app.process_command(UiCommand::Complete);
    assert!(app.messages().is_empty(), "not the shown chat's to end");

    app.process_command(for_agent("bb", tool_data("t1", "list_files")));
    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    let [Activity::Tool(tool)] = activities(&app)[..] else {
        panic!("expected one tool block");
    };
    assert_eq!(tool.policy.as_deref(), Some(FULL_ACCESS));
    assert_eq!(app.messages().len(), 1, "and no policy line");
}
