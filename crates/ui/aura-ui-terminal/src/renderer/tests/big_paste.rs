//! A big paste in the input: whole, fast, sent whole; the cursor staying
//! put while the box scrolls; Ctrl+Up/Down to the input's ends.

use super::input_box::{numbered_rows, shows_row};
use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEventKind};

/// About 50,000 chars on 1,000 lines, each telling its number.
fn big_paste() -> String {
    (1..=1_000)
        .map(|i| format!("row {i:02} the quick brown fox jumps over the lazy dog"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn key(app: &mut crate::App, code: KeyCode, modifiers: KeyModifiers) {
    let _ = app.handle_key(KeyEvent::new(code, modifiers));
}

#[test]
fn test_a_50k_paste_lands_whole_and_is_sent_whole() {
    use crate::events::UiEvent;
    let mut app = app_with_model(None);
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    app.link.event_tx = Some(tx);
    let paste = big_paste();
    assert!(paste.len() > 50_000, "{}", paste.len());
    app.handle_paste(&paste);
    assert!(app.input() == paste, "byte for byte");
    assert_eq!(app.input.cursor_pos, paste.len());
    let notice = app.notifications.current.clone().map(|(text, _)| text);
    assert!(
        notice.as_deref().is_some_and(|n| n.contains("1,000 lines")),
        "{notice:?}"
    );
    let rows = render_rows(&mut app, 120);
    assert!(
        shows_row(&rows, 1000) && !shows_row(&rows, 1),
        "the end shows"
    );

    key(&mut app, KeyCode::Enter, KeyModifiers::empty());
    let sent: Vec<UiEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    let text = sent.iter().find_map(|event| match event {
        UiEvent::UserMessage { text, .. } => Some(text.clone()),
        _ => None,
    });
    assert!(text.as_deref() == Some(paste.as_str()), "sent uncut");
}

#[test]
fn test_a_paste_past_the_limit_is_refused_whole() {
    let mut app = app_with_model(None);
    type_text(&mut app, "keep me");
    let huge = "x".repeat(crate::app::paste::MAX_INPUT_BYTES);
    app.handle_paste(&huge);
    assert_eq!(app.input(), "keep me", "nothing inserted, nothing cut");
    let notice = app.notifications.current.clone().map(|(text, _)| text);
    assert!(
        notice
            .as_deref()
            .is_some_and(|n| n.contains("Nothing was pasted")),
        "{notice:?}"
    );
}

#[test]
fn test_up_through_a_long_draft_stops_at_its_start() {
    let mut app = app_with_model(None);
    type_text(&mut app, "an older message");
    key(&mut app, KeyCode::Enter, KeyModifiers::empty());
    let paste = big_paste();
    app.handle_paste(&paste);
    render_rows(&mut app, 120);
    // Every wrapped row and then some: the box wraps each line in two.
    let rows = crate::input_wrap::row_starts(&paste, app.areas.input_wrap.unwrap()).len();
    for _ in 0..rows + 50 {
        key(&mut app, KeyCode::Up, KeyModifiers::empty());
    }
    assert!(app.input() == paste, "the draft was swapped for history");
    assert_eq!(app.input.cursor_pos, 0);
    // The box follows the cursor to the top. (Its cursor is hidden while
    // the sent message is "Thinking", so the drawn rows tell.)
    let rows = render_rows(&mut app, 120);
    assert!(shows_row(&rows, 1), "{rows:#?}");
    assert_eq!(app.areas.input_rows.map(|r| r.top), Some(0));

    // A one-row draft still recalls history.
    key(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
    key(&mut app, KeyCode::Char('k'), KeyModifiers::CONTROL);
    type_text(&mut app, "short");
    key(&mut app, KeyCode::Up, KeyModifiers::empty());
    assert_eq!(app.input(), "an older message");
}

#[test]
fn test_wheeling_a_long_draft_leaves_the_cursor_alone() {
    let mut app = app_with_model(None);
    app.handle_paste(&numbered_rows(40));
    render_rows(&mut app, 120);
    let cursor = app.input.cursor_pos;
    let input = app.areas.input.expect("input drawn");
    for _ in 0..50 {
        mouse_at(&mut app, MouseEventKind::ScrollUp, (input.x + 4, input.y));
        let (rows, (_, cursor_y)) = render_with_cursor(&mut app, 120);
        assert_eq!(app.input.cursor_pos, cursor);
        assert_ne!(usize::from(cursor_y), rows.len() - INPUT_FROM_BOTTOM);
    }
    let rows = render_rows(&mut app, 120);
    assert!(shows_row(&rows, 1), "scrolled to the top");
}

#[test]
fn test_ctrl_up_and_down_jump_to_the_input_ends() {
    let mut app = app_with_model(None);
    let paste = big_paste();
    app.handle_paste(&paste);
    render_rows(&mut app, 120);
    key(&mut app, KeyCode::Up, KeyModifiers::CONTROL);
    assert_eq!(app.input.cursor_pos, 0, "one key, not one row");
    let rows = render_rows(&mut app, 120);
    assert!(shows_row(&rows, 1) && !shows_row(&rows, 1000), "{rows:#?}");
    key(&mut app, KeyCode::Down, KeyModifiers::CONTROL);
    assert_eq!(app.input.cursor_pos, paste.len());
    let rows = render_rows(&mut app, 120);
    assert!(shows_row(&rows, 1000) && !shows_row(&rows, 1), "{rows:#?}");
    assert!(app.input() == paste, "only the cursor moved");

    // After the wheel moved the view, Ctrl+Down still brings it back.
    let input = app.areas.input.expect("input drawn");
    for _ in 0..20 {
        mouse_at(&mut app, MouseEventKind::ScrollUp, (input.x + 4, input.y));
    }
    render_rows(&mut app, 120);
    key(&mut app, KeyCode::Down, KeyModifiers::CONTROL);
    let rows = render_rows(&mut app, 120);
    assert!(shows_row(&rows, 1000), "{rows:#?}");
}

#[test]
fn test_only_the_rows_on_screen_are_cut_out() {
    use ratatui::text::Span;
    let text = numbered_rows(1_000);
    let starts = crate::input_wrap::row_starts(&text, 40);
    let spans = [Span::raw(text.as_str())];
    let rows = super::super::input::split_rows(&spans, &starts, 500..508);
    assert_eq!(rows.len(), 8);
    let first: String = rows[0].iter().map(|span| span.content.as_ref()).collect();
    assert_eq!(first, "row 501");
    let past: Vec<_> = super::super::input::split_rows(&spans, &starts, 999..1_003);
    assert_eq!(past.len(), 4, "rows past the text are empty, not missing");
    assert!(past[1].is_empty());
}
