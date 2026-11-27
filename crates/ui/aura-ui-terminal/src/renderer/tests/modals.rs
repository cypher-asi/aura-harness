//! The modal layer: the screen around the modal on top goes dark, with
//! nothing underneath showing through, not even the input's cursor, and
//! the modal shows no key hints.

use super::*;
use crate::components::{Message, MessageRole};
use crate::events::UiCommand;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, layout::Position, Terminal};

/// What opens a dialog on an app.
type OpenDialog = fn(&mut crate::App);

/// A 120 × 30 terminal with `app` drawn on it once.
fn drawn(app: &mut crate::App) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal
        .draw(|frame| super::super::render(frame, app, &test_theme()))
        .unwrap();
    terminal
}

fn screen_text(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn ask_approval(app: &mut crate::App) {
    app.process_command(UiCommand::RequestApproval {
        id: "req-1".to_string(),
        tool: "run_command".to_string(),
        description: "cargo test".to_string(),
        always: String::new(),
    });
}

#[test]
fn the_screen_around_a_modal_is_dark_and_blank() {
    let theme = test_theme();
    let mut app = app_with_model(None);
    app.add_message(Message::new(MessageRole::Assistant, "zebra crossing"));
    assert!(screen_text(&drawn(&mut app)).contains("zebra crossing"));

    ask_approval(&mut app);
    let terminal = drawn(&mut app);
    let frame = app.modal_area().expect("the approval is drawn").frame;
    let buffer = terminal.backend().buffer();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            let cell = &buffer[(x, y)];
            if !frame.contains(Position::new(x, y)) {
                assert_eq!(
                    (cell.symbol(), cell.bg),
                    (" ", theme.colors.backdrop),
                    "({x}, {y})"
                );
            }
        }
    }
    let text = screen_text(&terminal);
    assert!(!text.contains("zebra"), "{text}");
    assert!(text.contains("APPROVAL REQUIRED"), "{text}");
}

#[test]
fn the_input_cursor_hides_under_a_modal() {
    let mut app = crate::App::new();
    let mut terminal = drawn(&mut app);
    let origin = Position::new(0, 0);
    assert_ne!(
        terminal.get_cursor_position().unwrap(),
        origin,
        "the input shows its cursor"
    );

    run_slash(&mut app, "/help");
    let mut terminal = drawn(&mut app);
    assert_eq!(
        terminal.get_cursor_position().unwrap(),
        origin,
        "nothing places the cursor"
    );
}

#[test]
fn dialogs_show_no_key_hints() {
    let dialogs: [(&str, OpenDialog); 4] = [
        ("approval", ask_approval),
        ("model selector", |app| run_slash(app, "/model")),
        ("folder picker", |app| {
            app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
        }),
        ("channel form", crate::App::start_new_channel),
    ];
    for (name, open) in dialogs {
        let mut app = app_with_model(None);
        open(&mut app);
        assert!(app.modal().is_some(), "{name} opens");
        let text = screen_text(&drawn(&mut app));
        for hint in ["Esc", "Enter", "Press any key", "to filter", "submit"] {
            assert!(!text.contains(hint), "{name} shows {hint:?}:\n{text}");
        }
    }
}

#[test]
fn a_notice_under_a_modal_is_its_status() {
    let mut app = app_with_model(None);
    run_slash(&mut app, "/model");
    app.notify_copied(4, &Ok(()));
    let rows = render_rows(&mut app, 120);
    let notice = rows
        .iter()
        .position(|row| row.contains("✓ Copied 4 characters"))
        .expect("the notice shows");
    let bottom = rows
        .iter()
        .rposition(|row| row.contains('└'))
        .expect("the selector's box");
    assert!(notice < bottom, "inside the box: {}", rows.join("\n"));
}
