use super::*;

/// The tab row of the header bar holding `label`, and the widths of the
/// bar's cells between its frame and dividers. Checks that the bar is
/// framed, with each divider joining the frame, and that the panel's body
/// starts right below it.
pub(super) fn header_bar(rows: &[String], label: &str) -> (usize, Vec<usize>) {
    let grid: Vec<Vec<char>> = rows.iter().map(|r| r.chars().collect()).collect();
    let tabs = rows
        .iter()
        .position(|r| r.contains(label))
        .unwrap_or_else(|| panic!("no {label} in:\n{}", rows.join("\n")));
    let at = rows[tabs][..rows[tabs].find(label).unwrap()]
        .chars()
        .count();
    // The bar's left frame, not the divider of a tab before `label`.
    let left = (0..=at)
        .rev()
        .find(|&x| grid[tabs][x] == '│' && grid[tabs - 1][x] == '┌')
        .unwrap_or_else(|| panic!("no frame left of {label} in:\n{}", rows.join("\n")));
    let right = left
        + grid[tabs - 1][left..]
            .iter()
            .position(|&c| c == '┐')
            .unwrap();

    assert_eq!(grid[tabs - 1][left], '┌');
    assert_eq!(grid[tabs + 1][left], '└');
    assert_eq!(grid[tabs + 1][right], '┘');
    assert_eq!(grid[tabs + 2][left], '┌', "body right below the bar");
    let mut widths = Vec::new();
    let mut start = left + 1;
    for x in left + 1..=right {
        if grid[tabs][x] == '│' {
            if x < right {
                assert_eq!(grid[tabs - 1][x], '┬', "column {x}");
                assert_eq!(grid[tabs + 1][x], '┴', "column {x}");
            }
            widths.push(x - start);
            start = x + 1;
        }
    }
    (tabs, widths)
}

pub(super) fn submit(app: &mut crate::App, text: &str) {
    use crossterm::event::{KeyCode, KeyEvent};
    for c in text.chars() {
        let _ = app.handle_key(KeyEvent::from(KeyCode::Char(c)));
    }
    let _ = app.handle_key(KeyEvent::from(KeyCode::Enter));
}

pub(super) fn render_with_user(user: Option<&str>, width: u16) -> Vec<String> {
    render_rows(&mut app_with_model(user), width)
}

pub(super) fn app_with_model(user: Option<&str>) -> crate::App {
    app_with_auth(user, &[])
}

pub(super) fn app_with_auth(user: Option<&str>, plans: &[&str]) -> crate::App {
    use crate::events::{ModelOptionView, UiCommand};

    let mut app = crate::App::new();
    app.set_shell_prompt("[n3o@arch-aura aura-harness]$ ");
    app.process_command(UiCommand::SetModels {
        options: vec![ModelOptionView {
            id: "local/qwen3:4b".to_string(),
            label: "qwen3:4b".to_string(),
            local: true,
            installed: true,
            group: "Local (no login)".to_string(),
            provider: None,
            available: true,
            context_window: None,
        }],
        current: "local/qwen3:4b".to_string(),
        route: None,
        user: user.map(str::to_string),
        plans: plans.iter().map(|p| (*p).to_string()).collect(),
        thinking: None,
    });
    app.process_command(UiCommand::SetApiStatus {
        url: Some("http://127.0.0.1:8080".to_string()),
        active: true,
    });
    app
}

/// Rows from the bottom of the screen to a one-row input: below it are the
/// input box's bottom border and the footer's two rows.
pub(super) const INPUT_FROM_BOTTOM: usize = 3;

pub(super) fn render_rows(app: &mut crate::App, width: u16) -> Vec<String> {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 30)).unwrap();
    terminal
        .draw(|frame| super::super::render(frame, app, &test_theme()))
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

/// The chat panel's header tab row and its left and right border columns,
/// found from its CHAT | TERMINAL header bar (TRM when the bar is narrow).
pub(super) fn chat_panel(rows: &[String]) -> (usize, usize, usize) {
    let tabs = rows
        .iter()
        .position(|r| r.contains("CHAT") && (r.contains("TERMINAL") || r.contains(" TRM ")))
        .unwrap_or_else(|| panic!("no chat header bar in:\n{}", rows.join("\n")));
    let row: Vec<char> = rows[tabs].chars().collect();
    let chat = rows[tabs][..rows[tabs].find("CHAT").unwrap()]
        .chars()
        .count();
    let left = row[..=chat].iter().rposition(|&c| c == '│').unwrap();
    let right = left
        + 1
        + rows[tabs - 1]
            .chars()
            .skip(left + 1)
            .position(|c| c == '┐')
            .unwrap();
    (tabs, left, right)
}

pub(super) fn type_text(app: &mut crate::App, text: &str) {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    for c in text.chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
    }
}

/// `chars[from..to]` as a string.
pub(super) fn columns(chars: &[char], from: usize, to: usize) -> String {
    chars[from..to].iter().collect()
}

pub(super) fn submit_message(app: &mut crate::App, text: &str) {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    for c in text.chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
}

pub(super) fn mouse_at(
    app: &mut crate::App,
    kind: crossterm::event::MouseEventKind,
    (column, row): (u16, u16),
) {
    app.handle_mouse(crossterm::event::MouseEvent {
        kind,
        column,
        row,
        modifiers: crossterm::event::KeyModifiers::empty(),
    });
}

/// Turn the wheel up over the chat panel until it shows its top.
pub(super) fn scroll_chat_to_top(app: &mut crate::App, on: &str) {
    let buffer = render_buffer(app, 120);
    let row = row_of(&buffer, on);
    for _ in 0..100 {
        mouse_at(app, crossterm::event::MouseEventKind::ScrollUp, (60, row));
    }
}

pub(super) fn render_buffer(app: &mut crate::App, width: u16) -> ratatui::buffer::Buffer {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 30)).unwrap();
    terminal
        .draw(|frame| super::super::render(frame, app, &test_theme()))
        .unwrap();
    terminal.backend().buffer().clone()
}

/// First row that contains `needle`.
pub(super) fn row_of(buffer: &ratatui::buffer::Buffer, needle: &str) -> u16 {
    (0..buffer.area.height)
        .find(|&y| find_cell(buffer, y, needle).is_some())
        .unwrap_or_else(|| panic!("{needle:?} not on screen"))
}

/// Column of `needle` in row `y`, and the cell style there.
pub(super) fn find_cell(
    buffer: &ratatui::buffer::Buffer,
    y: u16,
    needle: &str,
) -> Option<(u16, Style)> {
    let needle: Vec<String> = needle.chars().map(String::from).collect();
    let width = buffer.area.width;
    let n = u16::try_from(needle.len()).ok()?;
    let col = (0..width.saturating_sub(n)).find(|&x| {
        needle
            .iter()
            .zip(x..)
            .all(|(c, cx)| buffer[(cx, y)].symbol() == c)
    })?;
    Some((col, buffer[(col, y)].style()))
}

/// Blank rows the chat keeps below its last line, as it was last drawn.
pub(super) fn chat_room(app: &crate::App) -> usize {
    let text = app.areas.chat_text.expect("the chat is drawn");
    super::super::chat_panel::breathing_room(usize::from(text.height))
}

/// The right border of the body of the panel whose header bar holds
/// `title`, top to bottom, without its corners.
pub(super) fn right_border(rows: &[String], title: &str) -> String {
    let header = rows.iter().position(|r| r.contains(title)).unwrap();
    let start = rows[header][..rows[header].find(title).unwrap()]
        .chars()
        .count();
    let top = header + 2;
    let column = start
        + rows[top]
            .chars()
            .skip(start)
            .position(|c| c == '┐')
            .unwrap();
    rows[top + 1..]
        .iter()
        .map(|r| r.chars().nth(column).unwrap())
        .take_while(|&c| c != '┘')
        .collect()
}

pub(super) fn run_slash(app: &mut crate::App, command: &str) {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    type_text(app, command);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
}

pub(super) fn todo(content: &str, state: crate::events::TodoState) -> crate::events::TodoView {
    crate::events::TodoView {
        content: content.to_string(),
        state,
        assignee: None,
        task: None,
    }
}

// ========================================================================
// Folding agents over their spawns
// ========================================================================

/// Rows of the agents panel, its first 30 columns, for `rows`.
pub(super) fn agents_panel(rows: &[String]) -> Vec<String> {
    rows.iter()
        .map(|row| row.chars().take(30).collect())
        .collect()
}

/// The panel row naming `name`, which must be listed once.
pub(super) fn row_naming<'a>(panel: &'a [String], name: &str) -> &'a str {
    let found: Vec<&String> = panel.iter().filter(|row| row.contains(name)).collect();
    match found[..] {
        [row] => row,
        _ => panic!("{name:?} in {found:?}"),
    }
}

/// Whether `text` opens with the chat's braille spinner, which the
/// projects list shows beside a working agent, padded out to the icon column.
pub(super) fn is_aura_ring(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| ('\u{2801}'..='\u{28ff}').contains(&c))
        && chars.all(|c| c == ' ')
}

/// Index of the first of `rows` that contains `needle`.
pub(super) fn line_with(rows: &[String], needle: &str) -> usize {
    rows.iter()
        .position(|row| row.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} not in:\n{}", rows.join("\n")))
}

/// Column of `needle` in `row`, counted in characters.
pub(super) fn char_column(row: &str, needle: &str) -> usize {
    let i = row
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in {row:?}"));
    row[..i].chars().count()
}

/// Draw `app` and return the screen rows and where the cursor is.
pub(super) fn render_with_cursor(app: &mut crate::App, width: u16) -> (Vec<String>, (u16, u16)) {
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
