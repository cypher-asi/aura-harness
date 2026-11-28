use super::channels::channels_app;
use super::*;

/// Row of the to-do drawer's header, the row with the count and the
/// fold toggle at the right.
fn todo_drawer_header(rows: &[String]) -> Option<usize> {
    rows.iter()
        .rposition(|row| row.contains("active") && (row.contains(" —  │") || row.contains(" +  │")))
}

/// Whether any row above `end` shows a to-do's box.
fn todo_box_above(rows: &[String], end: usize) -> bool {
    rows[..end]
        .iter()
        .any(|row| row.contains('☐') || row.contains('◐') || row.contains('✓'))
}

#[test]
fn the_todo_list_shows_in_the_drawer_and_never_in_the_chat() {
    use crate::events::{TodoState, UiCommand};

    let mut app = crate::App::new();
    app.process_command(UiCommand::SetTodos(vec![
        todo("Read the parser", TodoState::Completed),
        todo("Write the lexer", TodoState::InProgress),
        todo("Run the tests", TodoState::Pending),
    ]));

    let rows = render_rows(&mut app, 160);
    let header = todo_drawer_header(&rows)
        .unwrap_or_else(|| panic!("no drawer while idle:\n{}", rows.join("\n")));
    assert!(
        rows[header].contains("⬢ 2 active · 1 done"),
        "{:?}",
        rows[header]
    );
    assert!(rows[header - 1].contains('├'), "{:?}", rows[header - 1]);
    assert!(!todo_box_above(&rows, header - 1), "{}", rows.join("\n"));
    assert!(rows[header + 1].contains("◐ Write the lexer"));

    submit_message(&mut app, "go on");
    let rows = render_rows(&mut app, 160);
    let screen = rows.join("\n");
    let header = todo_drawer_header(&rows).unwrap_or_else(|| panic!("no drawer:\n{screen}"));
    assert!(
        rows[header].contains("⬢ Working on 2 active · 1 done"),
        "{:?}",
        rows[header]
    );
    let thinking = rows
        .iter()
        .position(|row| row.contains("⬢ Thinking…"))
        .unwrap_or_else(|| panic!("no working line:\n{screen}"));
    assert!(
        thinking < header - 1,
        "the chat sits above the drawer:\n{screen}"
    );
    assert!(!todo_box_above(&rows, header - 1), "{screen}");

    app.process_command(UiCommand::SetTodos(Vec::new()));
    let rows = render_rows(&mut app, 160);
    assert!(todo_drawer_header(&rows).is_none(), "{}", rows.join("\n"));
}

#[test]
fn a_todo_list_with_every_item_closed_hides_the_drawer() {
    use crate::events::{TodoState, UiCommand};

    let mut app = crate::App::new();
    submit_message(&mut app, "go on");
    app.process_command(UiCommand::SetTodos(vec![
        todo("Read the parser", TodoState::Completed),
        todo("Drop the old lexer", TodoState::Cancelled),
    ]));
    let rows = render_rows(&mut app, 160);
    let screen = rows.join("\n");
    assert!(
        todo_drawer_header(&rows).is_none() && !screen.contains("Read the parser"),
        "a finished list does not show while the turn goes on: {screen}"
    );
    assert!(screen.contains("⬢ Thinking…"), "{screen}");

    // New items bring the drawer back.
    app.process_command(UiCommand::SetTodos(vec![
        todo("Read the parser", TodoState::Completed),
        todo("Run the tests", TodoState::Pending),
    ]));
    let rows = render_rows(&mut app, 160);
    let header =
        todo_drawer_header(&rows).unwrap_or_else(|| panic!("no drawer:\n{}", rows.join("\n")));
    assert!(
        rows[header].contains("⬢ Working on 1 active · 1 done"),
        "{:?}",
        rows[header]
    );
}

#[test]
fn test_todos_command_folds_the_drawer_and_brings_back_a_finished_list() {
    use crate::events::{TodoState, UiCommand};

    let mut app = crate::App::new();
    app.process_command(UiCommand::SetTodos(vec![
        todo("Read the parser", TodoState::Completed),
        todo("Write the lexer", TodoState::InProgress),
        todo("Run the tests", TodoState::Pending),
    ]));

    run_slash(&mut app, "/todos");
    let rows = render_rows(&mut app, 160);
    let screen = rows.join("\n");
    let header = todo_drawer_header(&rows).unwrap_or_else(|| panic!("no drawer:\n{screen}"));
    assert!(rows[header].contains(" +  │"), "folded: {:?}", rows[header]);
    assert!(!screen.contains("Run the tests"), "{screen}");
    assert!(
        rows[header + 1].contains('├'),
        "one row: {:?}",
        rows[header + 1]
    );

    run_slash(&mut app, "/todos");
    let screen = render_rows(&mut app, 160).join("\n");
    assert!(screen.contains("☐ Run the tests"), "open again: {screen}");

    // A finished list hides the drawer; `/todos` brings its closed items
    // back, its history open, and puts them away again.
    app.process_command(UiCommand::SetTodos(vec![
        todo("Read the parser", TodoState::Completed),
        todo("Drop the old lexer", TodoState::Cancelled),
    ]));
    assert!(todo_drawer_header(&render_rows(&mut app, 160)).is_none());
    run_slash(&mut app, "/todos");
    let rows = render_rows(&mut app, 160);
    let screen = rows.join("\n");
    assert!(todo_drawer_header(&rows).is_some(), "{screen}");
    assert!(screen.contains("▾ Task history"), "{screen}");
    assert!(screen.contains("✓ Read the parser  done"), "{screen}");
    assert!(
        screen.contains("⊘ Drop the old lexer  cancelled"),
        "{screen}"
    );
    run_slash(&mut app, "/todos");
    let rows = render_rows(&mut app, 160);
    assert!(todo_drawer_header(&rows).is_none(), "{}", rows.join("\n"));

    // A new list in progress ends the forcing: once it finishes, the
    // drawer hides by itself again.
    run_slash(&mut app, "/todos");
    app.process_command(UiCommand::SetTodos(vec![todo(
        "Run the tests",
        TodoState::InProgress,
    )]));
    app.process_command(UiCommand::SetTodos(vec![todo(
        "Run the tests",
        TodoState::Completed,
    )]));
    let rows = render_rows(&mut app, 160);
    assert!(todo_drawer_header(&rows).is_none(), "{}", rows.join("\n"));
}

#[test]
fn test_an_open_channel_shows_its_coordinators_todos_in_the_drawer() {
    use crate::events::{TodoState, UiCommand};

    let mut app = channels_app();
    assert!(app.channel_open(), "CHANNELS opens #zode");
    app.process_command(UiCommand::SetActiveAgent("co".to_string()));
    app.process_command(UiCommand::ForAgent {
        agent_id: "co".to_string(),
        command: Box::new(UiCommand::SetTodos(vec![
            todo("Hand the parser to the worker", TodoState::Completed),
            todo("Review the worker's patch", TodoState::InProgress),
        ])),
    });

    let rows = render_rows(&mut app, 160);
    let screen = rows.join("\n");
    let header = todo_drawer_header(&rows).unwrap_or_else(|| panic!("no drawer:\n{screen}"));
    assert!(
        rows[header].contains("⬢ 1 active · 1 done"),
        "{:?}",
        rows[header]
    );
    assert!(
        rows[header + 1].contains("◐ Review the worker's patch"),
        "{screen}"
    );
    assert!(
        !screen.contains("Hand the parser"),
        "done goes to the history: {screen}"
    );
    assert!(!todo_box_above(&rows, header - 1), "{screen}");
    let input = rows
        .iter()
        .position(|row| row.contains("Message #zode"))
        .unwrap_or_else(|| panic!("no input:\n{screen}"));
    assert!(header < input, "the drawer sits above the input: {screen}");

    // It folds like anywhere else.
    run_slash(&mut app, "/todos");
    let rows = render_rows(&mut app, 160);
    let header = todo_drawer_header(&rows).expect("folded drawer");
    assert!(rows[header].contains(" +  │"), "{:?}", rows[header]);

    // CHANNELS with no channel open shows no agent, so no drawer.
    app.close_channel();
    let rows = render_rows(&mut app, 160);
    assert!(todo_drawer_header(&rows).is_none(), "{}", rows.join("\n"));
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

#[test]
fn test_each_assigned_todo_names_its_member_with_its_status() {
    use crate::events::{TodoState, TodoView, UiCommand};

    let mut app = channels_app();
    let mut channels = app.channels().to_vec();
    channels[0].members[1].code = Some("WK7".to_string());
    channels[0].members[1].definition = Some("frontend-engineer".to_string());
    channels[0].members[1].running = true;
    app.process_command(UiCommand::SetChannels(channels));
    app.process_command(UiCommand::SetActiveAgent("co".to_string()));
    let given = |content: &str, state, to: &str| TodoView {
        assignee: Some(to.to_string()),
        ..todo(content, state)
    };
    app.process_command(UiCommand::ForAgent {
        agent_id: "co".to_string(),
        command: Box::new(UiCommand::SetTodos(vec![
            // Given by code, and by the permanent agent it was started as.
            given("Fix the redirect", TodoState::InProgress, "WK7"),
            given("Style the footer", TodoState::Pending, "frontend-engineer"),
            given("Read the parser", TodoState::Completed, "WK7"),
        ])),
    });
    let rows = render_rows(&mut app, 160);
    let screen = rows.join("\n");
    let row = |needle: &str| {
        rows.iter()
            .find(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("no {needle:?}:\n{screen}"))
            .clone()
    };
    let fixing = row("Fix the redirect");
    assert!(fixing.contains("→ WK7 · working 0s"), "{fixing}");
    assert!(
        row("Style the footer").contains("→ WK7 · working"),
        "{screen}"
    );
    assert!(
        row("Task history").contains("▸ Task history  1"),
        "{screen}"
    );
    assert!(!screen.contains("Read the parser"), "{screen}");
}
