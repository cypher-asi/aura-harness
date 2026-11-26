use super::*;

// ========================================================================
// Ctrl+G editor
// ========================================================================

fn ctrl_g(app: &mut App) -> std::path::PathBuf {
    app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
    app.take_editor_request()
        .expect("Ctrl+G asks for the editor")
}

#[test]
fn test_ctrl_g_edits_the_input_in_the_editor() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new().with_event_sender(tx);
    type_keys(&mut app, "draft");
    let path = ctrl_g(&mut app);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "draft");

    std::fs::write(&path, "line one\nline two\n\n").unwrap();
    app.editor_closed(path.clone(), &Ok(()));

    assert_eq!(app.input(), "line one\nline two");
    assert_eq!(app.cursor_pos(), app.input().len());
    assert!(!path.exists(), "the temp file is removed");
    assert!(rx.try_recv().is_err(), "not a permanent-agent edit");

    app.handle_key(KeyEvent::from(KeyCode::Enter));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::UserMessage { text, .. }) if text == "line one\nline two"
    ));
}

#[test]
fn test_a_failed_editor_keeps_the_input() {
    let mut app = App::new();
    type_keys(&mut app, "keep me");
    let path = ctrl_g(&mut app);
    std::fs::write(&path, "half-written").unwrap();
    app.editor_closed(path.clone(), &Err(std::io::Error::other("exit status: 1")));

    assert_eq!(app.input(), "keep me");
    assert!(app
        .notification()
        .unwrap()
        .0
        .contains("the editor failed: exit status: 1"));
    assert!(!path.exists());
}

#[test]
fn test_ctrl_g_with_empty_input_starts_an_empty_file() {
    let mut app = App::new();
    let path = ctrl_g(&mut app);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
    std::fs::write(&path, "@src/main.rs explain").unwrap();
    app.editor_closed(path, &Ok(()));
    assert_eq!(app.input(), "@src/main.rs explain");
}

fn new_record(app: &mut App, seq: u64) {
    app.process_command(UiCommand::NewRecord(record_summary(seq)));
}

use super::super::detail_tests::record as record_summary;

fn selected_seq(app: &App) -> u64 {
    app.selected_record_data().unwrap().seq
}

#[test]
fn test_records_list_oldest_first_and_follow_newest() {
    let mut app = App::new();
    for seq in 0..3 {
        new_record(&mut app, seq);
    }
    let order: Vec<u64> = app.records().iter().map(|r| r.seq).collect();
    assert_eq!(order, [0, 1, 2]);
    assert_eq!(selected_seq(&app), 2);

    new_record(&mut app, 3);
    assert_eq!(selected_seq(&app), 3);
}

#[test]
fn test_records_selection_moved_up_stays_put() {
    let mut app = App::new();
    for seq in 0..3 {
        new_record(&mut app, seq);
    }
    app.focus = PanelFocus::Records;
    app.handle_key(key(KeyCode::Up));
    new_record(&mut app, 3);
    assert_eq!(selected_seq(&app), 1);

    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Down));
    new_record(&mut app, 4);
    assert_eq!(selected_seq(&app), 4);
}

#[test]
fn test_records_cap_drops_oldest_and_keeps_selection() {
    let mut app = App::new();
    let cap = u64::try_from(MAX_RECORDS).unwrap();
    for seq in 0..cap {
        new_record(&mut app, seq);
    }
    app.selected_record = 10;
    new_record(&mut app, cap);
    assert_eq!(app.records().len(), MAX_RECORDS);
    assert_eq!(app.records().front().unwrap().seq, 1);
    assert_eq!(app.records().back().unwrap().seq, cap);
    assert_eq!(selected_seq(&app), 10);
}

fn set_records(app: &mut App, seqs: impl IntoIterator<Item = u64>) {
    let records = seqs.into_iter().map(record_summary).collect();
    app.process_command(UiCommand::SetRecords(records));
}

#[test]
fn test_set_records_replaces_existing_records_and_follows_newest() {
    let mut app = App::new();
    for seq in 0..5 {
        new_record(&mut app, seq);
    }
    app.focus = PanelFocus::Records;
    app.handle_key(key(KeyCode::Up));
    assert_eq!(selected_seq(&app), 3);

    set_records(&mut app, 100..103);
    let order: Vec<u64> = app.records().iter().map(|r| r.seq).collect();
    assert_eq!(order, [100, 101, 102]);
    assert_eq!(selected_seq(&app), 102);

    new_record(&mut app, 103);
    assert_eq!(selected_seq(&app), 103);
}

#[test]
fn test_set_records_empty_clears_records() {
    let mut app = App::new();
    new_record(&mut app, 1);
    set_records(&mut app, []);
    assert!(app.records().is_empty());
    assert!(app.selected_record_data().is_none());
}

#[test]
fn test_set_records_over_cap_keeps_newest() {
    let mut app = App::new();
    let cap = u64::try_from(MAX_RECORDS).unwrap();
    set_records(&mut app, 0..cap + 5);
    assert_eq!(app.records().len(), MAX_RECORDS);
    assert_eq!(app.records().front().unwrap().seq, 5);
    assert_eq!(app.records().back().unwrap().seq, cap + 4);
    assert_eq!(selected_seq(&app), cap + 4);
}

#[test]
fn test_records_cap_is_well_above_a_hundred() {
    let mut app = App::new();
    for seq in 0..150 {
        new_record(&mut app, seq);
    }
    assert_eq!(app.records().len(), 150);
}
