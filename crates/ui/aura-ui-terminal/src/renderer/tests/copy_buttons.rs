use super::*;

#[test]
fn clicking_copy_log_copies_the_log_and_says_so() {
    use crate::events::{LogRunView, LogView, UiCommand};
    use crossterm::event::{MouseButton, MouseEventKind};

    let mut app = crate::App::new();
    app.select_pane_tab(crate::PaneTab::Chat(crate::ChatTab::Log));
    app.process_command(UiCommand::LogView(Box::new(LogView {
        path: "/runs/r/log.json".into(),
        json: "{\"whole\":true}".into(),
        runs: vec![LogRunView {
            run_id: "r".into(),
            title: "fix it".into(),
            ..LogRunView::default()
        }],
        ..LogView::default()
    })));
    let buffer = render_buffer(&mut app, 120);
    let row = row_of(&buffer, "Copy log");
    let (column, _) = find_cell(&buffer, row, "Copy log").unwrap();
    mouse_at(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        (column, row),
    );
    mouse_at(
        &mut app,
        MouseEventKind::Up(MouseButton::Left),
        (column, row),
    );
    assert!(app.copy_pending(), "the click asked for a copy");
    let buffer = render_buffer(&mut app, 120);
    assert_eq!(
        app.take_copy_text(&buffer).as_deref(),
        Some("{\"whole\":true}")
    );
    app.notify_copied(14, &Ok(()));
    assert!(app
        .notification()
        .is_some_and(|(message, _)| message.contains("Copied the log")));
    let buffer = render_buffer(&mut app, 120);
    assert!(
        find_cell(&buffer, row, "✓ Copied").is_some(),
        "the button says so"
    );
}

#[test]
fn clicking_a_code_blocks_copy_button_copies_its_code() {
    use crate::components::{Message, MessageRole};
    use crossterm::event::{MouseButton, MouseEventKind};

    let code = "cargo test -p aura-ui-terminal\necho done";
    let mut app = crate::App::new();
    app.add_message(Message::new(MessageRole::User, "how do I test it?"));
    app.add_message(Message::new(
        MessageRole::Assistant,
        &format!("Run this:\n\n```bash\n{code}\n```\n\nThen check the output."),
    ));
    let buffer = render_buffer(&mut app, 120);
    let row = row_of(&buffer, "⧉ Copy");
    let (column, _) = find_cell(&buffer, row, "⧉ Copy").unwrap();

    // Hovering paints the button in the accent.
    mouse_at(&mut app, MouseEventKind::Moved, (column, row));
    let buffer = render_buffer(&mut app, 120);
    let (_, style) = find_cell(&buffer, row, "⧉ Copy").unwrap();
    assert_eq!(style.fg, Some(test_theme().colors.success));

    mouse_at(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        (column, row),
    );
    mouse_at(
        &mut app,
        MouseEventKind::Up(MouseButton::Left),
        (column, row),
    );
    assert!(app.copy_pending(), "the click asked for a copy");
    assert!(app.selection().is_none(), "the click started no selection");
    let buffer = render_buffer(&mut app, 120);
    assert_eq!(app.take_copy_text(&buffer).as_deref(), Some(code));
    app.notify_copied(code.chars().count(), &Ok(()));
    assert!(app
        .notification()
        .is_some_and(|(message, _)| message.contains("Copied the code")));
}

#[test]
fn a_copy_button_drawn_over_does_not_copy() {
    use crate::components::{Message, MessageRole};
    use crossterm::event::{MouseButton, MouseEventKind};

    let mut app = crate::App::new();
    app.add_message(Message::new(
        MessageRole::Assistant,
        "Run this:\n\n```bash\nls\n```",
    ));
    let mut buffer = render_buffer(&mut app, 120);
    let row = row_of(&buffer, "⧉ Copy");
    let (column, _) = find_cell(&buffer, row, "⧉ Copy").unwrap();
    // A menu drawn over the button, as the slash menu is after the chat.
    buffer.set_string(column, row, "/clear  ", ratatui::style::Style::default());
    app.drop_covered_code_copies(&buffer);
    mouse_at(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        (column, row),
    );
    assert!(!app.copy_pending(), "nothing to copy under the menu");
}
