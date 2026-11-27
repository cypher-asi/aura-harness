//! The to-do drawer, between the Chat body and the input, in the same
//! column of borders: the Chat body's bottom border is the line above it,
//! its own bottom border the line above whatever comes next.
//!
//! ```text
//! ├──────────────────────────────────────────────┤
//! │  ⬢ Working on 4 to-dos  1/4               —  │
//! │ ☒ Read the parser                            │
//! │ ▣ Write the lexer                            │
//! │ ☐ Run the tests                              │
//! │ +1 more                                      │
//! ├──────────────────────────────────────────────┤
//! │  → Add a follow-up                           │
//! ```
//!
//! The header keeps the side padding of a panel's headings, the items a
//! list's column at the left, and neither a blank row on top: the drawer
//! is as tall as its rows.
//!
//! The header gives the count, `done/total` and a toggle, `—` to fold
//! the drawer to the header, `+` to open it. Folded, the item in
//! progress follows the count, so the current step stays in sight. Open,
//! each item is one row. Past [`MAX_TODO_ROWS`] items, or past the rows
//! the screen leaves it, the rows on screen start at the item in progress
//! so it stays in view, and a `+N more` row counts the rest.
//!
//! The drawer records where it drew its header on the [`App`] for the
//! mouse.

use super::chrome::{border_color, PAD_SIDE};
use super::hud::{checklist_header, todo_line, truncate, MAX_TODO_ROWS};
use super::input::join_divider;
use super::text::display_width;
use crate::app::{AppState, PanelFocus};
use crate::events::TodoState;
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Rows the border adds: only the bottom one, since the Chat body's
/// bottom border sits on top.
const BORDER_ROWS: u16 = 1;
/// Narrowest the item in progress is shown in a folded header; narrower
/// room leaves it out.
const MIN_FOLDED_TEXT: usize = 8;

/// Rows the drawer wants: none when it has nothing to show, the header
/// alone when folded, else the header, the items on screen and a
/// `+N more` row when some are hidden.
#[must_use]
pub(super) fn height(app: &App) -> u16 {
    if !app.todo_drawer_shown() {
        return 0;
    }
    if app.todo_drawer_folded() {
        return BORDER_ROWS + 1;
    }
    let len = app.hud().todos.len();
    let shown = len.min(MAX_TODO_ROWS);
    let more = usize::from(len > shown);
    u16::try_from(shown + more)
        .unwrap_or(u16::MAX)
        .saturating_add(BORDER_ROWS + 1)
}

/// Render the drawer into `area`, which [`height`] sized, and record
/// where its header went.
pub(super) fn render(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    if !app.todo_drawer_shown() || area.height < BORDER_ROWS + 1 || area.width < 6 {
        app.set_todo_drawer_header(None);
        return;
    }
    let block = Block::default()
        .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
        .border_style(Style::default().fg(border_color(app.focus() == PanelFocus::Chat, theme)));
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    join_divider(frame, area);

    let header = Rect { height: 1, ..inner };
    frame.render_widget(Paragraph::new(header_line(app, inner.width, theme)), header);
    app.set_todo_drawer_header(Some(header));

    let room = usize::from(inner.height.saturating_sub(1));
    if app.todo_drawer_folded() || room == 0 {
        return;
    }
    let todos = &app.hud().todos;
    let len = todos.len();
    let mut shown = len.min(MAX_TODO_ROWS).min(room);
    if len > shown && shown == room && shown > 1 {
        // Keep a row to say how many are hidden.
        shown -= 1;
    }
    // Long lists start at the item being worked on, so it stays in view.
    let first = todos
        .iter()
        .position(|todo| todo.state == TodoState::InProgress)
        .map_or(0, |at| at.min(len - shown));
    // A column clear of each border.
    let text_room = usize::from(header.width.saturating_sub(2));
    let mut y = header.y + 1;
    for todo in todos.iter().skip(first).take(shown) {
        let mut line = todo_line(todo, text_room, theme);
        line.spans.insert(0, Span::raw(" "));
        frame.render_widget(Paragraph::new(line), Rect { y, ..header });
        y += 1;
    }
    let hidden = len - shown;
    if hidden > 0 && y < inner.bottom() {
        let more = Line::from(Span::styled(
            format!(" +{hidden} more"),
            Style::default().fg(theme.colors.muted),
        ));
        frame.render_widget(Paragraph::new(more), Rect { y, ..header });
    }
}

/// `  ⬢ Working on 3 to-dos  1/3` at the left, with the item in progress
/// when folded, and the toggle at the right.
fn header_line(app: &App, width: u16, theme: &Theme) -> Line<'static> {
    let hud = app.hud();
    let working = app.state() == AppState::Processing;
    let shimmer = (working && theme.animate_spinners).then(|| app.animation_frame());
    let pad = " ".repeat(usize::from(PAD_SIDE));
    let mut spans = vec![Span::raw(pad.clone())];
    spans.extend(checklist_header(hud, theme, working, shimmer).spans);
    let (done, total) = hud.todo_progress();
    spans.push(Span::styled(
        format!("  {done}/{total}"),
        Style::default().fg(theme.colors.muted),
    ));

    let folded = app.todo_drawer_folded();
    let toggle = if folded { "+" } else { "—" };
    let toggle_color = if app.todo_drawer_hovered() {
        theme.colors.foreground
    } else {
        theme.colors.muted
    };
    let width = usize::from(width);
    let used =
        |spans: &[Span<'_>]| -> usize { spans.iter().map(|s| display_width(&s.content)).sum() };
    // The toggle, a space before it and the closing pad.
    let tail = display_width(toggle) + 1 + pad.len();
    let current = hud
        .todos
        .iter()
        .find(|todo| todo.state == TodoState::InProgress)
        .filter(|_| folded);
    if let Some(todo) = current {
        let room = width.saturating_sub(used(&spans) + 2 + tail);
        if room >= MIN_FOLDED_TEXT {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                truncate(&todo.content.replace('\n', " "), room),
                Style::default().fg(theme.colors.foreground),
            ));
        }
    }
    let fill = width.saturating_sub(used(&spans) + display_width(toggle) + pad.len());
    spans.extend([
        Span::raw(" ".repeat(fill)),
        Span::styled(toggle, Style::default().fg(toggle_color)),
        Span::raw(pad),
    ]);
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use crate::events::{TodoState, TodoView, UiCommand};
    use crate::{App, Theme};
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

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
        rows.iter().enumerate().rev().find(|(_, row)| {
            row.contains("to-do") && (row.contains(" —  │") || row.contains(" +  │"))
        })
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
        assert!(row_with(&rows, "Read the parser").is_none());
    }

    #[test]
    fn the_drawer_lists_the_todos_between_the_chat_and_the_input() {
        let mut app = App::new();
        app.process_command(three_todos());
        let rows = rows(&mut app);

        let (at, header_row) = header(&rows).expect("header");
        assert!(header_row.contains("│  ⬢ 3 to-dos  1/3"), "{header_row:?}");
        assert!(!header_row.contains("Working"), "idle: {header_row:?}");
        assert!(rows[at - 1].contains('├'), "{:?}", rows[at - 1]);
        assert!(
            rows[at + 1].contains("│ ☒ Read the parser"),
            "{:?}",
            rows[at + 1]
        );
        assert!(
            rows[at + 2].contains("│ ▣ Write the lexer"),
            "{:?}",
            rows[at + 2]
        );
        assert!(
            rows[at + 3].contains("│ ☐ Run the tests"),
            "{:?}",
            rows[at + 3]
        );
        assert!(rows[at + 4].contains('├'), "{:?}", rows[at + 4]);
        let (prompt, _) = row_with(&rows, "→ ").expect("the input");
        assert!(prompt > at + 4, "the drawer sits above the input");
    }

    #[test]
    fn the_header_says_working_during_a_turn() {
        let mut app = App::new();
        app.process_command(three_todos());
        start_turn(&mut app);
        let rows = rows(&mut app);
        let (_, header_row) = header(&rows).expect("header");
        assert!(
            header_row.contains("⬢ Working on 3 to-dos  1/3"),
            "{header_row:?}"
        );
    }

    #[test]
    fn a_click_on_the_header_folds_the_drawer_to_one_row() {
        let mut app = App::new();
        app.process_command(three_todos());
        let before = rows(&mut app);
        let (at, header_row) = header(&before).expect("header");
        let column = header_row.chars().position(|c| c == '⬢').unwrap();
        let click = |app: &mut App, kind| {
            app.handle_mouse(MouseEvent {
                kind,
                column: u16::try_from(column).unwrap(),
                row: u16::try_from(at).unwrap(),
                modifiers: KeyModifiers::empty(),
            });
        };
        click(&mut app, MouseEventKind::Down(MouseButton::Left));
        click(&mut app, MouseEventKind::Up(MouseButton::Left));
        assert!(app.todo_drawer_folded());

        let rows = rows(&mut app);
        let (at, header_row) = header(&rows).expect("header");
        assert!(header_row.contains(" +  │"), "{header_row:?}");
        assert!(
            header_row.contains("1/3  Write the lexer"),
            "the item in progress follows the count: {header_row:?}"
        );
        assert!(rows[at + 1].contains('├'), "{:?}", rows[at + 1]);
        assert!(row_with(&rows, "Read the parser").is_none());

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
                        0..=11 => TodoState::Completed,
                        12 => TodoState::InProgress,
                        _ => TodoState::Pending,
                    };
                    todo(&format!("Step {i}"), state)
                })
                .collect(),
        )
    }

    #[test]
    fn a_long_list_keeps_the_item_in_progress_in_view() {
        let mut app = App::new();
        app.process_command(long_list());
        let rows = rows(&mut app);
        let (at, _) = header(&rows).expect("header");

        assert!(rows[at + 1].contains("☒ Step 5 "), "{:?}", rows[at + 1]);
        assert!(row_with(&rows, "▣ Step 12").is_some());
        assert!(rows[at + 10].contains("☐ Step 14"), "{:?}", rows[at + 10]);
        assert!(rows[at + 11].contains("│ +5 more"), "{:?}", rows[at + 11]);
        assert!(row_with(&rows, "Step 4 ").is_none());
    }

    #[test]
    fn a_short_screen_squeezes_the_drawer_and_counts_the_rest() {
        let mut app = App::new();
        app.process_command(long_list());
        let rows = rows_of(&mut app, 16);
        let (at, _) = header(&rows).expect("header");

        assert!(
            row_with(&rows, "▣ Step 12").is_some(),
            "{}",
            rows.join("\n")
        );
        let (more, more_row) = row_with(&rows, "more").expect("a +N more row");
        assert!(more > at, "{more_row:?}");
        let shown = more - at - 1;
        assert!(
            more_row.contains(&format!("+{} more", 15 - shown)),
            "{more_row:?}"
        );
        let (prompt, _) = row_with(&rows, "→ ").expect("the input");
        assert!(prompt > more, "the input keeps its place");
    }
}
