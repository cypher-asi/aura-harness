use super::*;

#[test]
fn test_todo_checklist_and_list() {
    use crate::events::{TodoState, UiCommand};

    let mut app = crate::App::new();
    app.process_command(UiCommand::SetTodos(vec![
        todo("Read the parser", TodoState::Completed),
        todo("Write the lexer", TodoState::InProgress),
        todo("Run the tests", TodoState::Pending),
    ]));

    let screen = render_rows(&mut app, 160).join("\n");
    assert!(
        !screen.contains("Working on"),
        "only while a turn runs: {screen}"
    );
    assert!(!screen.contains("TODOS"));

    run_slash(&mut app, "/todos");
    let screen = render_rows(&mut app, 160).join("\n");
    assert!(screen.contains("TODOS 1/3"), "{screen}");
    assert!(screen.contains("☒ Read the parser"));
    assert!(screen.contains("☐ Run the tests"));
    run_slash(&mut app, "/todos");

    submit_message(&mut app, "go on");
    let rows = render_rows(&mut app, 160);
    let row = |needle: &str| {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not in:\n{}", rows.join("\n")))
    };
    let header = row("⬢ Working on 3 to-dos");
    assert_eq!(row("☒ Read the parser"), header + 1);
    assert_eq!(row("▣ Write the lexer"), header + 2);
    assert_eq!(row("☐ Run the tests"), header + 3);
    assert_eq!(row("⬢ Thinking…"), header + 5);
    // The boxes line up under the header's `⬢`.
    let column = |row: usize, glyph: char| {
        rows[row]
            .chars()
            .position(|c| c == glyph)
            .unwrap_or_else(|| panic!("no {glyph} in {:?}", rows[row]))
    };
    let hex = column(header, '⬢');
    assert_eq!(column(header + 1, '☒'), hex);
    assert_eq!(column(header + 2, '▣'), hex);
    assert_eq!(column(header + 3, '☐'), hex);

    app.process_command(UiCommand::SetTodos(Vec::new()));
    let screen = render_rows(&mut app, 160).join("\n");
    assert!(!screen.contains("Working on"));
}

#[test]
fn a_todo_list_with_every_item_closed_leaves_the_live_tail() {
    use crate::events::{TodoState, UiCommand};

    let mut app = crate::App::new();
    submit_message(&mut app, "go on");
    app.process_command(UiCommand::SetTodos(vec![
        todo("Read the parser", TodoState::Completed),
        todo("Drop the old lexer", TodoState::Cancelled),
    ]));
    let screen = render_rows(&mut app, 160).join("\n");
    assert!(
        !screen.contains("Working on") && !screen.contains("Read the parser"),
        "a finished list does not show while the turn goes on: {screen}"
    );
    assert!(screen.contains("⬢ Thinking…"), "{screen}");

    // New items bring the checklist back.
    app.process_command(UiCommand::SetTodos(vec![
        todo("Read the parser", TodoState::Completed),
        todo("Run the tests", TodoState::Pending),
    ]));
    let screen = render_rows(&mut app, 160).join("\n");
    assert!(screen.contains("⬢ Working on 2 to-dos"), "{screen}");
}

#[test]
fn test_clicking_the_todo_checklist_lists_every_item() {
    use crate::events::{TodoState, UiCommand};
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    let mut app = crate::App::new();
    app.process_command(UiCommand::SetTodos(
        (0..12)
            .map(|i| todo(&format!("Step {i}"), TodoState::Pending))
            .collect(),
    ));
    submit_message(&mut app, "go on");
    let click_on = |app: &mut crate::App, needle: &str| {
        let rows = render_rows(app, 160);
        let row = rows
            .iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not in:\n{}", rows.join("\n")));
        let column = rows[row]
            .find(needle.trim())
            .map_or(0, |at| rows[row][..at].chars().count());
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.handle_mouse(MouseEvent {
                kind,
                column: u16::try_from(column).unwrap(),
                row: u16::try_from(row).unwrap(),
                modifiers: KeyModifiers::empty(),
            });
        }
    };

    let screen = render_rows(&mut app, 160).join("\n");
    assert!(screen.contains("2 more"), "{screen}");
    assert!(!screen.contains("Step 11"), "{screen}");

    click_on(&mut app, "2 more");
    assert!(app.todos_expanded());
    let screen = render_rows(&mut app, 160).join("\n");
    assert!(screen.contains("☐ Step 11"), "{screen}");
    assert!(screen.contains("show less"), "{screen}");
    assert!(!screen.contains("2 more"), "{screen}");

    click_on(&mut app, "show less");
    assert!(!app.todos_expanded());
    let screen = render_rows(&mut app, 160).join("\n");
    assert!(screen.contains("2 more"), "{screen}");
}

#[test]
fn test_context_and_cost_commands_write_to_the_chat() {
    use crate::events::{CallStats, ContextView, UiCommand};

    let mut app = crate::App::new();
    app.process_command(UiCommand::CallStats(CallStats {
        input_tokens: 2_000,
        cache_read_tokens: 8_000,
        output_tokens: 500,
        cost_usd: Some(0.02),
        context_limit: Some(100_000),
        ..CallStats::default()
    }));
    app.process_command(UiCommand::SetContext(ContextView {
        system_tokens: 3_000,
        tools_tokens: 4_000,
        conversation_tokens: 3_000,
        total_tokens: 10_000,
        limit_tokens: Some(100_000),
        ..ContextView::default()
    }));

    run_slash(&mut app, "/context");
    run_slash(&mut app, "/cost");
    let texts: Vec<String> = app
        .messages()
        .iter()
        .map(|m| m.content().to_string())
        .collect();
    assert!(texts[0].contains("10.0k of 100k tokens (10%)"), "{texts:?}");
    assert!(texts[0].contains("tools"), "{texts:?}");
    assert!(texts[0].contains("Prompt cache hits: 80%"), "{texts:?}");
    assert!(texts[1].contains("$0.020"), "{texts:?}");
}
