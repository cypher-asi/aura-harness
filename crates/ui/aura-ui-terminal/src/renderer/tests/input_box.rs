use super::*;

#[test]
fn test_prompt_colors_user_green_and_host_gray() {
    let theme = test_theme();
    let cyan = theme.colors.primary;
    let spans = super::super::input::prompt_spans("[n3o@arch-aura aura-harness]$ ", cyan, &theme);
    let parts: Vec<(&str, Option<Color>)> = spans
        .iter()
        .map(|s| (s.content.as_ref(), s.style.fg))
        .collect();
    assert_eq!(
        parts,
        [
            ("[", Some(cyan)),
            ("n3o", Some(theme.colors.success)),
            ("@arch-aura", Some(theme.colors.muted)),
            (" aura-harness]$ ", Some(cyan)),
        ]
    );

    let email = super::super::input::prompt_spans("Email: ", cyan, &theme);
    assert_eq!(email.len(), 1);
    assert_eq!(email[0].style.fg, Some(cyan));
}

#[test]
fn test_typing_sits_in_a_box_under_the_chat_panel() {
    let mut app = app_with_model(None);
    type_text(&mut app, "hello");
    let rows = render_rows(&mut app, 120);
    let input_row = &rows[rows.len() - INPUT_FROM_BOTTOM];

    let (_, chat_border, chat_right) = chat_panel(&rows);
    let chars: Vec<char> = input_row.chars().collect();
    let typing = chat_border + 5;
    let typed: String = chars[typing..typing + 5].iter().collect();
    let lead: String = chars[chat_border..typing].iter().collect();
    assert_eq!(typed, "hello", "{input_row:?}");
    assert_eq!(lead, "│  → ", "{input_row:?}");
    assert_eq!(chars[chat_right], '│', "{input_row:?}");
}

#[test]
fn test_long_input_wraps_inside_the_chat_and_grows_upward() {
    let mut app = app_with_model(None);
    let text = "word ".repeat(40);
    type_text(&mut app, text.trim_end());
    let (rows, (_, cursor_y)) = render_with_cursor(&mut app, 120);
    let n = rows.len();
    let (_, chat_left, chat_right) = chat_panel(&rows);

    assert!(rows[n - 1].contains("● Ready"), "{:?}", rows[n - 1]);
    let input_rows: Vec<&String> = rows[..n - 3]
        .iter()
        .rev()
        .take_while(|r| r.contains("word"))
        .collect();
    assert!(input_rows.len() > 1, "{rows:#?}");
    for row in &input_rows {
        let chars: Vec<char> = row.chars().collect();
        let beyond: String = chars[chat_right..].iter().collect();
        assert!(!beyond.contains("wo") && !beyond.contains("or"), "{row:?}");
        assert_eq!(chars[chat_left], '│', "{row:?}");
        assert_eq!(chars[chat_right], '│', "{row:?}");
    }
    assert_eq!(
        usize::from(cursor_y),
        n - INPUT_FROM_BOTTOM,
        "cursor on the last input row"
    );
}

#[test]
fn test_prompt_pushes_typing_right_without_gutter() {
    let mut app = app_with_model(None);
    submit_message(&mut app, "/swarm");
    type_text(&mut app, "hello");
    let rows = render_rows(&mut app, 120);
    let input_row = &rows[rows.len() - INPUT_FROM_BOTTOM];

    assert!(input_row.starts_with(" │  → hello"), "{input_row:?}");
}

#[test]
fn test_busy_input_shows_a_stop_button_at_the_right_edge() {
    let mut app = app_with_model(None);
    submit_message(&mut app, "hi");
    assert!(app.is_busy());
    let rows = render_rows(&mut app, 120);
    let y = rows.len() - INPUT_FROM_BOTTOM;
    let (_, chat_left, chat_right) = chat_panel(&rows);
    let chars: Vec<char> = rows[y].chars().collect();
    let in_box = columns(&chars, chat_left, chat_right + 1);
    assert!(in_box.ends_with(" ■ Stop  │"), "{in_box:?}");
    assert!(in_box.starts_with("│  → "), "{in_box:?}");

    let stop = app.input_stop_area().expect("the button's cells");
    assert_eq!((usize::from(stop.y), stop.width, stop.height), (y, 6, 1));
    let (x, end) = (usize::from(stop.x), usize::from(stop.right()));
    assert_eq!(columns(&chars, x, end), "■ Stop");
    assert_eq!(
        end + 2,
        chat_right,
        "two columns of padding before the border"
    );
}

#[test]
fn test_idle_input_has_no_stop_button() {
    let mut app = app_with_model(None);
    let screen = render_rows(&mut app, 120).join("\n");
    assert!(!screen.contains("Stop"), "{screen}");
    assert_eq!(app.input_stop_area(), None);

    // It comes with a turn and goes with it.
    submit_message(&mut app, "hi");
    render_rows(&mut app, 120);
    assert!(app.input_stop_area().is_some());
    app.cancel();
    let screen = render_rows(&mut app, 120).join("\n");
    assert!(!screen.contains("■ Stop"), "{screen}");
    assert_eq!(app.input_stop_area(), None);
}

#[test]
fn test_typing_beside_the_stop_button_wraps_before_it() {
    let mut app = app_with_model(None);
    submit_message(&mut app, "hi");
    let typed = "abcdefghij".repeat(10);
    type_text(&mut app, &typed);
    let rows = render_rows(&mut app, 120);
    let last = rows.len() - INPUT_FROM_BOTTOM;
    let (_, chat_left, chat_right) = chat_panel(&rows);
    let typing = chat_left + 5;
    let stop = app.input_stop_area().expect("the button's cells");
    let (x, y) = (usize::from(stop.x), usize::from(stop.y));
    assert!(y < last, "the text wraps onto more rows: {rows:#?}");

    let first: Vec<char> = rows[y].chars().collect();
    assert_eq!(columns(&first, x, x + 6), "■ Stop", "{:?}", rows[y]);
    assert_eq!(first[x - 1], ' ', "a gap before the button");
    assert!(first[x - 2].is_ascii_lowercase(), "{:?}", rows[y]);

    let mut shown = String::new();
    for row in &rows[y..=last] {
        let chars: Vec<char> = row.chars().collect();
        shown += columns(&chars, typing, x - 1).trim_end();
        if row != &rows[y] {
            let beyond = columns(&chars, x - 1, chat_right);
            assert!(beyond.trim().is_empty(), "under the button: {row:?}");
        }
    }
    assert_eq!(shown, typed, "every letter is drawn left of the button");
}

#[test]
fn test_typed_newlines_grow_the_input_box_a_row_each() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let area = Rect::new(0, 0, 60, 20);
    let shift_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
    let mut app = app_with_model(None);
    type_text(&mut app, "a");
    assert_eq!(super::super::input::input_rows(area, &app, 20), 1);
    for c in ["b", "c"] {
        app.handle_key(shift_enter);
        type_text(&mut app, c);
    }
    assert_eq!(app.input(), "a\nb\nc");
    assert_eq!(super::super::input::input_rows(area, &app, 20), 3);

    let (rows, (x, y)) = render_with_cursor(&mut app, 120);
    let n = rows.len();
    let (_, chat_left, chat_right) = chat_panel(&rows);
    let typing = chat_left + 5;
    let in_box = |row: &String| -> String {
        let chars: Vec<char> = row.chars().collect();
        columns(&chars, chat_left, chat_right + 1)
    };
    assert!(in_box(&rows[n - 5]).starts_with("│  → a "), "{rows:#?}");
    assert!(in_box(&rows[n - 4]).starts_with("│    b "), "{rows:#?}");
    assert!(in_box(&rows[n - 3]).starts_with("│    c "), "{rows:#?}");
    let divider: Vec<char> = rows[n - 6].chars().collect();
    assert_eq!(
        (divider[chat_left], divider[chat_right]),
        ('├', '┤'),
        "the box starts three rows up: {:?}",
        rows[n - 6]
    );
    assert_eq!((usize::from(x), usize::from(y)), (typing + 1, n - 3));

    // A trailing break opens an empty row with the cursor at its start.
    app.handle_key(shift_enter);
    assert_eq!(super::super::input::input_rows(area, &app, 20), 4);
    let (rows, (x, y)) = render_with_cursor(&mut app, 120);
    let n = rows.len();
    assert!(in_box(&rows[n - 4]).starts_with("│    c "), "{rows:#?}");
    assert!(in_box(&rows[n - 3]).trim_matches('│').trim().is_empty());
    assert_eq!((usize::from(x), usize::from(y)), (typing, n - 3));
    assert!(!rows.join("\n").contains('↵'));
}

/// Shift+Enter on an empty prompt opens a second row, wherever the focus
/// is and whether or not a turn is running.
#[test]
fn test_shift_enter_on_an_empty_prompt_grows_the_box() {
    use crate::app::PanelFocus;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let area = Rect::new(0, 0, 60, 20);
    let shift_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
    for focus in [
        PanelFocus::Chat,
        PanelFocus::Swarm,
        PanelFocus::Records,
        PanelFocus::Sandbox,
    ] {
        for busy in [false, true] {
            let mut app = app_with_model(None);
            if busy {
                submit_message(&mut app, "hi");
                assert!(app.is_busy());
            }
            render_with_cursor(&mut app, 120);
            tab_to(&mut app, focus);
            assert_eq!(super::super::input::input_rows(area, &app, 20), 1);
            let (before, _) = render_with_cursor(&mut app, 120);

            app.handle_key(shift_enter);
            assert_eq!(app.input(), "\n", "{focus:?} busy={busy}");
            assert_eq!(
                super::super::input::input_rows(area, &app, 20),
                2,
                "{focus:?} busy={busy}"
            );
            app.handle_key(shift_enter);
            assert_eq!(super::super::input::input_rows(area, &app, 20), 3);

            let (rows, (_, y)) = render_with_cursor(&mut app, 120);
            assert_eq!(
                divider_row(&before) - divider_row(&rows),
                2,
                "the box grows two rows: {rows:#?}"
            );
            // The cursor hides while the agent thinks.
            if focus == PanelFocus::Chat && !busy {
                assert_eq!(usize::from(y), divider_row(&rows) + 3, "busy={busy}");
            }
            assert!(!rows.join("\n").contains("Add a follow-up"));
            assert!(!rows.join("\n").contains("Ask anything"));
        }
    }
}

/// Press Tab until `focus` has the focus.
fn tab_to(app: &mut crate::App, focus: crate::app::PanelFocus) {
    use crossterm::event::{KeyCode, KeyEvent};

    for _ in 0..4 {
        if app.focus() == focus {
            break;
        }
        app.handle_key(KeyEvent::from(KeyCode::Tab));
    }
    assert_eq!(app.focus(), focus);
}

/// The row of the divider above the prompt box.
fn divider_row(rows: &[String]) -> usize {
    let (_, chat_left, _) = chat_panel(rows);
    rows.iter()
        .rposition(|row| row.chars().nth(chat_left) == Some('├'))
        .unwrap()
}

/// Draw `app` and return the screen rows and where the cursor is.
fn render_with_cursor(app: &mut crate::App, width: u16) -> (Vec<String>, (u16, u16)) {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 30)).unwrap();
    terminal
        .draw(|frame| super::super::render(frame, app, &test_theme()))
        .unwrap();
    let cursor = terminal.get_cursor_position().unwrap();
    let buffer = terminal.backend().buffer();
    let rows = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect();
    (rows, (cursor.x, cursor.y))
}

/// A draft of `n` rows, "row 1" to "row n", one per line.
fn numbered_rows(n: usize) -> String {
    (1..=n)
        .map(|i| format!("row {i:02}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether the screen shows the draft row `row NN`.
fn shows_row(rows: &[String], i: usize) -> bool {
    let label = format!("row {i:02}");
    rows.iter().any(|r| r.contains(&label))
}

#[test]
fn test_wheel_scrolls_an_overflowing_input_and_clamps_at_its_ends() {
    use crossterm::event::MouseEventKind;

    let mut app = app_with_model(None);
    app.handle_paste(&numbered_rows(20));
    let rows = render_rows(&mut app, 120);
    assert!(shows_row(&rows, 20) && !shows_row(&rows, 1), "{rows:#?}");
    let input = app.areas.input.expect("input drawn");
    let over = (input.x + 4, input.y);
    let drawn = app.areas.input_rows.expect("rows recorded");
    assert!(drawn.total > drawn.visible, "{drawn:?}");
    let max_top = drawn.total - drawn.visible;
    assert_eq!(drawn.top, max_top, "follows the cursor on the last row");

    mouse_at(&mut app, MouseEventKind::ScrollUp, over);
    let (rows, (_, cursor_y)) = render_with_cursor(&mut app, 120);
    assert_eq!(app.areas.input_rows.map(|r| r.top), Some(max_top - 1));
    assert!(!shows_row(&rows, 20), "the last row scrolled out");
    assert_ne!(
        usize::from(cursor_y),
        rows.len() - INPUT_FROM_BOTTOM,
        "the cursor is not drawn on a row it is not on"
    );

    for _ in 0..30 {
        mouse_at(&mut app, MouseEventKind::ScrollUp, over);
    }
    let rows = render_rows(&mut app, 120);
    assert_eq!(app.areas.input_rows.map(|r| r.top), Some(0), "clamps at 0");
    assert!(shows_row(&rows, 1), "{rows:#?}");

    for _ in 0..30 {
        mouse_at(&mut app, MouseEventKind::ScrollDown, over);
    }
    let rows = render_rows(&mut app, 120);
    assert_eq!(app.areas.input_rows.map(|r| r.top), Some(max_top));
    assert!(shows_row(&rows, 20), "{rows:#?}");
    assert_eq!(app.input(), numbered_rows(20), "scrolling leaves the draft");
}

#[test]
fn test_typing_after_scrolling_the_input_brings_the_cursor_back() {
    use crossterm::event::MouseEventKind;

    let mut app = app_with_model(None);
    app.handle_paste(&numbered_rows(20));
    render_rows(&mut app, 120);
    let input = app.areas.input.expect("input drawn");
    for _ in 0..30 {
        mouse_at(&mut app, MouseEventKind::ScrollUp, (input.x + 4, input.y));
    }
    let rows = render_rows(&mut app, 120);
    assert!(shows_row(&rows, 1), "{rows:#?}");

    type_text(&mut app, "!");
    let (rows, (_, cursor_y)) = render_with_cursor(&mut app, 120);
    assert!(rows.iter().any(|r| r.contains("row 20!")), "{rows:#?}");
    assert!(!shows_row(&rows, 1), "{rows:#?}");
    assert_eq!(usize::from(cursor_y), rows.len() - INPUT_FROM_BOTTOM);
}

#[test]
fn test_wheel_over_a_short_input_leaves_it_alone() {
    use crossterm::event::MouseEventKind;

    let mut app = app_with_model(None);
    type_text(&mut app, "hello");
    let before = render_rows(&mut app, 120);
    let input = app.areas.input.expect("input drawn");
    mouse_at(&mut app, MouseEventKind::ScrollUp, (input.x + 4, input.y));
    assert_eq!(app.input_scroll_top(), None);
    let after = render_rows(&mut app, 120);
    let n = before.len();
    assert_eq!(
        before[n - INPUT_FROM_BOTTOM],
        after[n - INPUT_FROM_BOTTOM],
        "the input row is unchanged"
    );
}

#[test]
fn test_clicking_the_input_box_focuses_the_chat_panel() {
    use crate::app::PanelFocus;
    use crossterm::event::{MouseButton, MouseEventKind};

    let mut app = app_with_model(None);
    app.focus = PanelFocus::Swarm;
    render_rows(&mut app, 120);
    let input = app.areas.input.expect("input drawn");
    mouse_at(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        (input.x + 4, input.y + 1),
    );
    assert_eq!(app.focus(), PanelFocus::Chat);
}

#[test]
fn test_terminal_types_at_the_input_line_after_the_shell_prompt() {
    use crossterm::event::{KeyCode, KeyEvent};

    let mut app = app_with_model(None);
    type_text(&mut app, "$ls");
    let (rows, (x, y)) = render_with_cursor(&mut app, 120);

    let (_, chat_left, _) = chat_panel(&rows);
    let input_y = rows.len() - INPUT_FROM_BOTTOM;
    let typed: String = rows[input_y].chars().skip(chat_left + 3).collect();
    let prompt = "[n3o@arch-aura aura-harness]$ ";
    assert!(typed.starts_with(&format!("{prompt}ls")), "{typed:?}");
    assert_eq!(
        (usize::from(x), usize::from(y)),
        (chat_left + 3 + prompt.len() + 2, input_y)
    );
    let pane = rows[..input_y].join("\n");
    assert!(!pane.contains("]$ "), "{pane}");
    assert!(!pane.contains("Esc returns to chat"), "no keys: {pane}");

    app.handle_key(KeyEvent::from(KeyCode::Backspace));
    app.handle_key(KeyEvent::from(KeyCode::Backspace));
    app.handle_key(KeyEvent::from(KeyCode::Backspace));
    let rows = render_rows(&mut app, 120);
    let input_row = &rows[rows.len() - INPUT_FROM_BOTTOM];
    assert!(input_row.contains("→ "), "{input_row:?}");
    assert!(!rows.join("\n").contains("]$ "));
}

#[test]
fn test_terminal_input_is_highlighted_as_a_command() {
    let theme = test_theme();
    let mut app = app_with_model(None);
    type_text(&mut app, "$ls -la");
    let buffer = render_buffer(&mut app, 120);
    let y = buffer.area.height - 3;

    let (_, program) = find_cell(&buffer, y, "ls -la").unwrap();
    assert_eq!(program.fg, Some(theme.colors.success));
    let (_, flag) = find_cell(&buffer, y, "-la").unwrap();
    assert_eq!(flag.fg, Some(theme.colors.warning));
}
