use super::file_view::FileBody;
use super::*;
use crate::events::{RecordStatus, UiCommand};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::layout::Rect;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::empty())
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::empty(),
    }
}

fn record(seq: u64) -> RecordSummary {
    RecordSummary {
        seq,
        timestamp: "12:00:00".to_string(),
        full_hash: String::new(),
        hash_suffix: String::new(),
        tx_kind: "Prompt".to_string(),
        sender: String::new(),
        message: String::new(),
        action_count: 0,
        effect_status: String::new(),
        status: RecordStatus::Ok,
        info: String::new(),
        error_details: String::new(),
        tx_id: String::new(),
        agent_id: String::new(),
        ts_ms: 0,
        policy: None,
    }
}

fn app_with_sandbox() -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("notes.md"), "# Notes\n").unwrap();
    let mut app = App::new();
    app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
    (dir, app)
}

#[test]
fn enter_on_record_opens_it_and_esc_closes() {
    let mut app = App::new();
    app.process_command(UiCommand::NewRecord(record(1)));
    app.focus = PanelFocus::Records;

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.detail_view(), Some(&DetailView::Record));

    app.set_detail_bounds(10, 4);
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::PageDown));
    assert_eq!(app.detail_scroll(), 5);
    app.handle_key(key(KeyCode::End));
    app.set_detail_bounds(10, 4);
    assert_eq!(app.detail_scroll(), 10);
    app.handle_key(key(KeyCode::Home));
    assert_eq!(app.detail_scroll(), 0);

    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.detail_view(), None);
    assert_eq!(app.focus(), PanelFocus::Records);
}

#[test]
fn typing_while_open_does_not_reach_the_input() {
    let mut app = App::new();
    app.process_command(UiCommand::NewRecord(record(1)));
    app.focus = PanelFocus::Records;
    app.handle_key(key(KeyCode::Enter));

    app.handle_key(key(KeyCode::Char('x')));
    assert!(app.input().is_empty());
    app.handle_key(key(KeyCode::Char('q')));
    assert_eq!(app.detail_view(), None);
}

#[test]
fn enter_on_sandbox_file_opens_it_but_folders_still_expand() {
    let (_dir, mut app) = app_with_sandbox();
    app.focus = PanelFocus::Sandbox;

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.detail_view(), None);
    assert!(app.sandbox().rows()[0].expanded);

    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));
    let Some(DetailView::File(file)) = app.detail_view() else {
        panic!("expected a file view, got {:?}", app.detail_view());
    };
    assert_eq!(file.title, "notes.md");
    assert_eq!(file.body, FileBody::Text("# Notes\n".to_string()));
}

#[test]
fn clicking_a_sandbox_file_opens_or_replaces_the_view() {
    let (dir, mut app) = app_with_sandbox();
    std::fs::write(dir.path().join("other.txt"), "x").unwrap();
    app.sandbox.refresh();
    app.areas.sandbox = Some(Rect::new(80, 10, 30, 10));
    app.set_sandbox_rows_area(Rect::new(81, 11, 28, 8), 0);

    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 85, 12));
    let title = |app: &App| match app.detail_view() {
        Some(DetailView::File(file)) => file.title.clone(),
        other => panic!("expected a file view, got {other:?}"),
    };
    assert_eq!(title(&app), "notes.md");

    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 85, 13));
    assert_eq!(title(&app), "other.txt");
}

#[test]
fn wheel_over_chat_scrolls_the_open_view() {
    let (_dir, mut app) = app_with_sandbox();
    app.areas.chat = Some(Rect::new(0, 0, 60, 20));
    app.focus = PanelFocus::Sandbox;
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));

    app.handle_mouse(mouse(MouseEventKind::ScrollDown, 10, 10));
    assert_eq!(app.detail_scroll(), 3);
    assert_eq!(app.scroll_offset, 0);
}

#[test]
fn clearing_records_closes_only_a_record_view() {
    let (_dir, mut app) = app_with_sandbox();
    app.process_command(UiCommand::NewRecord(record(1)));
    app.focus = PanelFocus::Sandbox;
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));
    app.process_command(UiCommand::ClearRecords);
    assert!(matches!(app.detail_view(), Some(DetailView::File(_))));

    app.handle_key(key(KeyCode::Esc));
    app.process_command(UiCommand::NewRecord(record(2)));
    app.focus = PanelFocus::Records;
    app.handle_key(key(KeyCode::Enter));
    app.process_command(UiCommand::ClearRecords);
    assert_eq!(app.detail_view(), None);
}
