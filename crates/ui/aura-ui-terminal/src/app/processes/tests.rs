
use super::*;
use crate::app::PanelFocus;
use crate::events::{ProcessKind, ProcessStatus, UiCommand};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use tokio::sync::mpsc;

fn process(id: &str, status: ProcessStatus, killable: bool) -> ProcessSummary {
    ProcessSummary {
        id: id.into(),
        kind: ProcessKind::Command,
        label: format!("job {id}"),
        status,
        owner_agent_id: "owner".into(),
        owner_name: "Agent".into(),
        jump_agent_id: "root".into(),
        killable,
        ..ProcessSummary::default()
    }
}

fn app() -> (App, mpsc::Receiver<UiEvent>) {
    let (tx, rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    app.process_command(UiCommand::SetProcesses(vec![
        process("a", ProcessStatus::Running, true),
        process("b", ProcessStatus::Running, false),
        process("c", ProcessStatus::Exited, true),
    ]));
    app.focus = PanelFocus::Sandbox;
    app.show_side_tab(SidePanelTab::Processes);
    (app, rx)
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::from(code)
}

#[test]
fn shift_arrows_switch_tabs_and_plain_arrows_stay_in_the_list() {
    let mut app = App::new();
    app.focus = PanelFocus::Sandbox;
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    assert_eq!(app.side_tab(), SidePanelTab::Processes);
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.side_tab(), SidePanelTab::Processes);
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
    assert_eq!(app.side_tab(), SidePanelTab::Sandbox);
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
    assert_eq!(app.side_tab(), SidePanelTab::Tasks);
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
    assert_eq!(app.side_tab(), SidePanelTab::Trace);
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
    assert_eq!(app.side_tab(), SidePanelTab::Host);
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
    assert_eq!(app.side_tab(), SidePanelTab::Processes);
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.side_tab(), SidePanelTab::Processes);
    for _ in 0..4 {
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    }
    assert_eq!(app.side_tab(), SidePanelTab::Sandbox);
}

#[test]
fn home_and_end_move_the_list_not_the_input() {
    let (mut app, _rx) = app();
    app.handle_key(key(KeyCode::End));
    assert_eq!(app.selected_process(), 2);
    app.handle_key(key(KeyCode::Home));
    assert_eq!(app.selected_process(), 0);
    assert!(app.input().is_empty());
}

#[test]
fn selection_follows_the_process_across_updates() {
    let (mut app, _rx) = app();
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.selected_process_data().unwrap().id, "b");
    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(app.detail_view(), Some(DetailView::Process(p)) if p.id == "b"));

    let mut finished = process("b", ProcessStatus::Killed, false);
    finished.exit_code = Some(137);
    app.process_command(UiCommand::SetProcesses(vec![
        process("new", ProcessStatus::Running, true),
        process("a", ProcessStatus::Running, true),
        finished,
    ]));
    assert_eq!(app.selected_process(), 2);
    let Some(DetailView::Process(open)) = app.detail_view() else {
        panic!("process view closed");
    };
    assert_eq!(open.status, ProcessStatus::Killed);
    assert_eq!(open.exit_code, Some(137));
}

#[test]
fn kill_asks_first_and_only_for_live_killable_processes() {
    let (mut app, mut rx) = app();
    app.handle_key(key(KeyCode::Char('k')));
    assert_eq!(app.pending_kill.as_deref(), Some("a"));
    app.handle_key(key(KeyCode::Char('y')));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::KillProcess(id)) if id == "a"));

    app.handle_key(key(KeyCode::Char('k')));
    app.handle_key(key(KeyCode::Char('n')));
    assert!(rx.try_recv().is_err());
    assert!(app.input().is_empty());

    for index in [1, 2] {
        app.processes.selected = index;
        app.handle_key(key(KeyCode::Delete));
        assert!(app.pending_kill.is_none());
        assert!(app.notification().is_some());
    }
}

#[test]
fn o_opens_the_agent_at_the_root_of_the_process() {
    let (mut app, mut rx) = app();
    app.handle_key(key(KeyCode::Char('o')));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "root"));
}

#[test]
fn clicking_tabs_and_rows() {
    let (mut app, _rx) = app();
    app.areas.sandbox = Some(Rect::new(80, 10, 30, 12));
    app.set_side_tabs_area(Rect::new(81, 11, 28, 1));
    app.set_process_rows_area(Rect::new(81, 13, 28, 8), 0);
    let click = |column, row| MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::empty(),
    };

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        ..click(82, 14)
    });
    assert_eq!(app.hover(), Some(HoverTarget::ProcessRow(1)));
    app.handle_mouse(click(82, 14));
    assert_eq!(app.selected_process(), 1);
    assert!(
        matches!(app.detail_view(), Some(DetailView::Process(p)) if p.id == "b"),
        "one click opens the process"
    );

    app.close_detail();
    app.handle_mouse(click(82, 11));
    assert_eq!(app.side_tab(), SidePanelTab::Sandbox);
}

#[test]
fn trace_rows_hover_and_open_on_one_click() {
    use crate::events::{TraceNode, TraceNodeStatus};
    let mut app = App::new();
    app.trace_feed.push(
        "",
        "r",
        &[TraceNode {
            id: "s1".into(),
            kind: "iteration".into(),
            label: "step".into(),
            status: TraceNodeStatus::Ok,
            ..TraceNode::default()
        }],
    );
    app.close_detail();
    app.show_side_tab(SidePanelTab::Trace);
    app.areas.sandbox = Some(Rect::new(80, 10, 30, 12));
    app.set_trace_rows_area(Rect::new(81, 13, 28, 8), 0);
    assert_eq!(
        app.hovered_side_panel(82, 13),
        Some(HoverTarget::TraceRow(0))
    );
    app.click_side_panel(82, 13);
    assert!(matches!(app.detail_view(), Some(DetailView::File(_))));
}
