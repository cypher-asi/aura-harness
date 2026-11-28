use crate::events::{TodoState, TodoView, UiCommand};
use crate::{App, Theme};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

fn todo(content: &str, state: TodoState) -> TodoView {
    TodoView {
        content: content.to_string(),
        state,
        assignee: None,
        task: None,
    }
}

fn three_todos() -> UiCommand {
    UiCommand::SetTodos(vec![
        todo("Read the parser", TodoState::Completed),
        todo("Write the lexer", TodoState::InProgress),
        todo("Run the tests", TodoState::Pending),
    ])
}

fn rows_of(app: &mut App, height: u16) -> Vec<String> {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(160, height)).unwrap();
    terminal
        .draw(|frame| super::super::render(frame, app, &Theme::cyber()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect()
}

fn rows(app: &mut App) -> Vec<String> {
    rows_of(app, 30)
}

/// The drawer's header: the last row with its toggle.
fn header(rows: &[String]) -> Option<(usize, &String)> {
    rows.iter()
        .enumerate()
        .rev()
        .find(|(_, row)| row.contains("active") && (row.contains(" —  │") || row.contains(" +  │")))
}

fn row_with<'a>(rows: &'a [String], needle: &str) -> Option<(usize, &'a String)> {
    rows.iter()
        .enumerate()
        .rev()
        .find(|(_, row)| row.contains(needle))
}

fn start_turn(app: &mut App) {
    for c in "go on".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
}

/// Press the left button on the cell where `needle` starts on its row.
fn click_on(app: &mut App, rows: &[String], needle: &str) {
    let (at, row) = row_with(rows, needle).unwrap_or_else(|| panic!("no {needle:?}"));
    let column = row.find(needle).map_or(0, |x| row[..x].chars().count());
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        app.handle_mouse(MouseEvent {
            kind,
            column: u16::try_from(column).unwrap(),
            row: u16::try_from(at).unwrap(),
            modifiers: KeyModifiers::empty(),
        });
    }
}

fn alt_h(app: &mut App) {
    app.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::ALT));
}

#[test]
fn no_drawer_without_open_todos() {
    let mut app = App::new();
    assert!(header(&rows(&mut app)).is_none());
    app.process_command(UiCommand::SetTodos(vec![
        todo("Read the parser", TodoState::Completed),
        todo("Drop the old lexer", TodoState::Cancelled),
    ]));
    assert!(!app.todo_drawer_shown());
    let rows = rows(&mut app);
    assert!(header(&rows).is_none(), "{}", rows.join("\n"));
}

#[test]
fn the_drawer_lists_only_open_items_and_folds_the_rest_into_history() {
    let mut app = App::new();
    app.process_command(three_todos());
    let rows = rows(&mut app);
    let (at, header_row) = header(&rows).expect("header");
    assert!(header_row.contains("⬢ 2 active · 1 done"), "{header_row:?}");
    assert!(rows[at - 1].contains('├'), "{:?}", rows[at - 1]);
    let expected = [
        "│ ◐ Write the lexer",
        "│   In progress",
        "│ ☐ Run the tests",
        "│   Pending",
        "│ ▸ Finished to-dos  1",
    ];
    for (row, expected) in rows[at + 1..].iter().zip(expected) {
        assert!(row.contains(expected), "{expected:?} in {row:?}");
    }
    assert!(rows[at + 6].contains('├'), "{:?}", rows[at + 6]);
    assert!(
        row_with(&rows, "Read the parser").is_none(),
        "done stays in the history"
    );
    let (prompt, _) = row_with(&rows, "→ ").expect("the input");
    assert!(prompt > at + 6, "the drawer sits above the input");
}

#[test]
fn the_header_says_working_during_a_turn() {
    let mut app = App::new();
    app.process_command(three_todos());
    start_turn(&mut app);
    let rows = rows(&mut app);
    let (_, header_row) = header(&rows).expect("header");
    assert!(
        header_row.contains("⬢ Working on 2 active · 1 done"),
        "{header_row:?}"
    );
}

#[test]
fn a_click_on_the_history_row_opens_and_folds_it() {
    let mut app = App::new();
    app.process_command(UiCommand::SetTodos(vec![
        todo("Read the parser", TodoState::Completed),
        todo("Write the lexer", TodoState::InProgress),
        todo("Drop the old lexer", TodoState::Cancelled),
    ]));
    let before = rows(&mut app);
    assert!(!app.todo_history_open(), "folded by default");
    click_on(&mut app, &before, "Finished to-dos");
    assert!(app.todo_history_open());
    let open = rows(&mut app);
    let (at, _) = row_with(&open, "▾ Finished to-dos").expect("open history");
    let expected = [
        "│ ✓ Read the parser",
        "│   Done",
        "│ ⊘ Drop the old lexer",
        "│   Cancelled",
    ];
    for (row, expected) in open[at + 1..].iter().zip(expected) {
        assert!(row.contains(expected), "{expected:?} in {row:?}");
    }
    assert!(!app.todo_drawer_folded(), "the drawer itself stays open");
    click_on(&mut app, &open, "Finished to-dos");
    assert!(!app.todo_history_open());
    assert!(row_with(&rows(&mut app), "Read the parser").is_none());
}

#[test]
fn alt_h_and_the_todos_command_open_and_fold_the_history() {
    let mut app = App::new();
    app.process_command(three_todos());
    alt_h(&mut app);
    assert!(app.todo_history_open());
    assert!(row_with(&rows(&mut app), "✓ Read the parser").is_some());
    alt_h(&mut app);
    assert!(!app.todo_history_open());

    for c in "/todos history".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
    assert!(app.todo_history_open());

    // Nothing finished: no history to open, and the key is not used.
    app.process_command(UiCommand::SetTodos(vec![todo(
        "Run the tests",
        TodoState::Pending,
    )]));
    app.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::ALT));
    assert!(row_with(&rows(&mut app), "Finished to-dos").is_none());
}

#[test]
fn a_click_on_the_header_folds_the_drawer_to_one_row() {
    let mut app = App::new();
    app.process_command(three_todos());
    let before = rows(&mut app);
    click_on(&mut app, &before, "⬢");
    assert!(app.todo_drawer_folded());

    let rows = rows(&mut app);
    let (at, header_row) = header(&rows).expect("header");
    assert!(header_row.contains(" +  │"), "{header_row:?}");
    assert!(
        header_row.contains("1 done  Write the lexer"),
        "the item in progress follows the count: {header_row:?}"
    );
    assert!(rows[at + 1].contains('├'), "{:?}", rows[at + 1]);
    assert!(row_with(&rows, "Finished to-dos").is_none());

    // A new empty list opens the drawer for the next one.
    app.process_command(UiCommand::SetTodos(Vec::new()));
    assert!(!app.todo_drawer_folded());
}

#[test]
fn hovering_the_header_lights_its_toggle() {
    let mut app = App::new();
    app.process_command(three_todos());
    let rows = rows(&mut app);
    let (at, header_row) = header(&rows).expect("header");
    let column = header_row.chars().position(|c| c == '⬢').unwrap();
    let moved = app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column: u16::try_from(column).unwrap(),
        row: u16::try_from(at).unwrap(),
        modifiers: KeyModifiers::empty(),
    });
    assert!(moved, "a hover that lights the toggle redraws");
    assert!(app.todo_drawer_hovered());
}

fn long_list() -> UiCommand {
    UiCommand::SetTodos(
        (0..15)
            .map(|i| {
                let state = match i {
                    0..=2 => TodoState::Completed,
                    3 => TodoState::InProgress,
                    _ => TodoState::Pending,
                };
                todo(&format!("Step {i}"), state)
            })
            .collect(),
    )
}

#[test]
fn a_long_list_keeps_the_item_in_progress_and_the_history_in_view() {
    let mut app = App::new();
    app.process_command(long_list());
    let rows = rows(&mut app);
    let (at, header_row) = header(&rows).expect("header");
    assert!(header_row.contains("12 active · 3 done"), "{header_row:?}");
    assert!(rows[at + 1].contains("◐ Step 3 "), "{:?}", rows[at + 1]);
    assert!(rows[at + 2].contains("In progress"), "{:?}", rows[at + 2]);
    assert!(rows[at + 7].contains("☐ Step 6 "), "{:?}", rows[at + 7]);
    assert!(
        rows[at + 9].contains("▸ Finished to-dos  3"),
        "{:?}",
        rows[at + 9]
    );
    assert!(rows[at + 10].contains("│ +8 more"), "{:?}", rows[at + 10]);
    assert!(
        rows[at + 11].contains('├'),
        "no blank row under the count: {:?}",
        rows[at + 11]
    );
    assert!(row_with(&rows, "Step 0 ").is_none());
    assert!(row_with(&rows, "Step 7 ").is_none());
}

#[test]
fn a_short_screen_squeezes_the_drawer_and_counts_the_rest() {
    let mut app = App::new();
    app.process_command(long_list());
    let rows = rows_of(&mut app, 16);
    let (at, _) = header(&rows).expect("header");
    assert!(row_with(&rows, "◐ Step 3").is_some(), "{}", rows.join("\n"));
    let (more, more_row) = row_with(&rows, "more").expect("a +N more row");
    assert!(more > at, "{more_row:?}");
    // An item's first row has its marker; the history row is one entry too.
    let shown = rows[at + 1..more]
        .iter()
        .filter(|row| row.contains('◐') || row.contains('☐') || row.contains("Finished to-dos"))
        .count();
    assert!(
        more_row.contains(&format!("+{} more", 13 - shown)),
        "{more_row:?}"
    );
    let (prompt, _) = row_with(&rows, "→ ").expect("the input");
    assert!(prompt > more, "the input keeps its place");
}

/// Four items still open and 29 finished, each given to `RUS3`.
fn busy_list() -> UiCommand {
    UiCommand::SetTodos(
        (0..33)
            .map(|i| {
                let state = match i {
                    0..=28 => TodoState::Completed,
                    29 => TodoState::InProgress,
                    _ => TodoState::Pending,
                };
                TodoView {
                    assignee: Some("RUS3".to_string()),
                    ..todo(&format!("Step {i}"), state)
                }
            })
            .collect(),
    )
}

fn wheel(app: &mut App, (column, row): (usize, usize), kind: MouseEventKind) {
    app.handle_mouse(MouseEvent {
        kind,
        column: u16::try_from(column).unwrap(),
        row: u16::try_from(row).unwrap(),
        modifiers: KeyModifiers::empty(),
    });
}

#[test]
fn an_opened_history_lists_its_items_even_under_four_open_ones() {
    let mut app = App::new();
    app.process_command(busy_list());
    let folded = rows(&mut app);
    assert!(row_with(&folded, "▸ Finished to-dos  29").is_some());
    click_on(&mut app, &folded, "Finished to-dos");
    let open = rows(&mut app);
    let (at, _) = row_with(&open, "▾ Finished to-dos").expect("the history row first");
    assert!(open[at + 1].contains("│ ✓ Step 0 "), "{}", open.join("\n"));
    assert!(open[at + 2].contains("RUS3 · done"), "{:?}", open[at + 2]);
    assert!(open[at + 3].contains("✓ Step 1 "), "{:?}", open[at + 3]);
    let (more, more_row) = row_with(&open, "more").expect("the rest counted");
    assert!(more > at, "{more_row:?}");
    let x = open[at].chars().position(|c| c == '▾').unwrap();

    // The wheel over the rows scrolls them, two items down.
    for _ in 0..2 {
        wheel(&mut app, (x, at + 2), MouseEventKind::ScrollDown);
    }
    let scrolled = rows(&mut app);
    assert!(row_with(&scrolled, "▾ Finished to-dos").is_none());
    let (head, _) = header(&scrolled).expect("header");
    let first = &scrolled[head + 1];
    assert!(first.contains("✓ Step 1 "), "step 1 first: {first:?}");

    // Far past the end it stops at the last page, the last item in view.
    for _ in 0..60 {
        wheel(&mut app, (x, at + 2), MouseEventKind::ScrollDown);
    }
    let end = rows(&mut app);
    assert!(row_with(&end, "✓ Step 28 ").is_some(), "{}", end.join("\n"));
    wheel(&mut app, (x, at + 2), MouseEventKind::ScrollUp);
    let back = rows(&mut app);
    assert!(
        row_with(&back, "✓ Step 27 ").is_some(),
        "one notch back from the end"
    );
}

#[test]
fn the_wheel_over_a_folded_history_is_the_chats() {
    let mut app = App::new();
    app.process_command(busy_list());
    let before = rows(&mut app);
    let (at, row) = header(&before).expect("header");
    let x = row.chars().position(|c| c == '⬢').unwrap();
    wheel(&mut app, (x, at + 2), MouseEventKind::ScrollDown);
    assert_eq!(rows(&mut app), before, "the drawer did not move");
}
