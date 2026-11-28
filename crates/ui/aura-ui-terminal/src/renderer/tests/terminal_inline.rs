//! The terminal inline in the chat panel: no input box under it, the
//! prompt following the output, and what is typed going to that prompt.

use super::*;
use crate::events::{UiCommand, UiEvent};
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEventKind};

/// The column of the chat panel's left border.
fn chat_left(rows: &[String]) -> usize {
    chat_panel(rows).1
}

/// The symbol in `row` at `column`.
fn at(rows: &[String], row: usize, column: usize) -> Option<char> {
    rows[row].chars().nth(column)
}

#[test]
fn the_terminal_has_no_input_box_and_chat_gets_it_back() {
    let mut app = app_with_model(None);
    let rows = render_rows(&mut app, 120);
    let left = chat_left(&rows);
    let input = rows.len() - INPUT_FROM_BOTTOM;
    assert_eq!(
        at(&rows, input - 1, left),
        Some('├'),
        "chat: the box's divider"
    );
    assert!(rows[input].contains("→ "), "{:?}", rows[input]);

    type_text(&mut app, "$");
    let rows = render_rows(&mut app, 120);
    let screen = rows.join("\n");
    assert!(
        (0..rows.len()).all(|y| at(&rows, y, left) != Some('├')),
        "no box joined under the terminal:\n{screen}"
    );
    // The panel runs down to the row the box's border was on.
    assert_eq!(at(&rows, input, left), Some('│'), "{screen}");
    assert_eq!(at(&rows, input + 1, left), Some('└'), "{screen}");
    assert!(
        !rows[input].contains("]$ "),
        "the prompt is not at the bottom:\n{screen}"
    );

    app.handle_key(KeyEvent::from(KeyCode::Esc));
    let rows = render_rows(&mut app, 120);
    assert_eq!(app.chat_tab(), crate::app::ChatTab::Chat);
    assert_eq!(at(&rows, input - 1, left), Some('├'), "the box is back");
    assert!(rows[input].contains("→ "), "{:?}", rows[input]);
}

#[test]
fn typing_goes_to_the_prompt_after_the_output_and_enter_runs_it() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let mut app = app_with_model(None).with_event_sender(tx);
    app.set_shell_prompt("$ ");
    submit_message(&mut app, "$echo hi");
    app.process_command(UiCommand::CommandFinished {
        output: "hi\n".to_string(),
        exit_code: None,
    });
    app.process_command(UiCommand::Complete);
    type_text(&mut app, "ls");

    let (rows, (x, y)) = render_with_cursor(&mut app, 120);
    let echo = rows
        .iter()
        .position(|row| row.contains("$ echo hi"))
        .unwrap();
    assert!(rows[echo + 1].contains("hi"), "{:?}", rows[echo + 1]);
    let typed = &rows[echo + 2];
    let prompt = typed.find("$ ls").expect("the prompt under the output");
    let prompt_x = typed[..prompt].chars().count();
    assert_eq!(usize::from(y), echo + 2, "the cursor is on the prompt");
    assert_eq!(usize::from(x), prompt_x + "$ ls".len());
    assert_eq!(
        prompt_x,
        rows[echo][..rows[echo].find("$ echo").unwrap()]
            .chars()
            .count(),
        "the prompt lines up with the output"
    );

    while rx.try_recv().is_ok() {}
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    let ran = std::iter::from_fn(|| rx.try_recv().ok()).find_map(|event| match event {
        UiEvent::RunCommand { command, .. } => Some(command),
        _ => None,
    });
    assert_eq!(ran.as_deref(), Some("ls"));
    let rows = render_rows(&mut app, 120);
    let ls = rows.iter().position(|row| row.contains("$ ls")).unwrap();
    assert_eq!(ls, echo + 2, "the command stays where it was typed");
    assert!(rows[ls + 1].contains("$ "), "the next prompt under it");
    assert!(!rows[ls + 1].contains("$ ls"), "{:?}", rows[ls + 1]);
}

#[test]
fn the_pointer_finds_the_input_on_the_inline_prompt() {
    let mut app = app_with_model(None);
    app.set_shell_prompt("$ ");
    type_text(&mut app, "$");
    let buffer = render_buffer(&mut app, 120);
    let row = row_of(&buffer, "$ ");
    let (column, _) = find_cell(&buffer, row, "$ ").unwrap();
    let input = app.areas.input.expect("the input was drawn");
    assert_eq!(input.y, row, "the input's area is the prompt's row");
    assert!(input.height < 3, "{input:?}");

    // A right click pastes there, as it does in the input box.
    mouse_at(
        &mut app,
        MouseEventKind::Down(MouseButton::Right),
        (column + 3, row),
    );
    assert!(app.take_paste_request());
}
