//! Working a tab's UI as the pointer would, for the tests here and the
//! shell's.

use aura_ui_terminal::App;
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

/// `tui`, drawn 120 columns by 30 rows, a line per row.
pub fn screen(tui: &mut App) -> Vec<String> {
    use ratatui::{backend::TestBackend, Terminal};
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).expect("never fails");
    let theme = aura_ui_terminal::Theme::cyber();
    terminal
        .draw(|frame| aura_ui_terminal::render(frame, tui, &theme))
        .expect("a test backend draws");
    let buffer = terminal.backend().buffer();
    let width = usize::from(buffer.area.width);
    buffer
        .content
        .chunks(width)
        .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
        .collect()
}

/// The cell where `text` first shows in `rows`.
pub fn cell_of(rows: &[String], text: &str) -> (u16, u16) {
    let row = rows
        .iter()
        .position(|row| row.contains(text))
        .unwrap_or_else(|| panic!("no {text}:\n{}", rows.join("\n")));
    let line = &rows[row];
    let column = line[..line.find(text).unwrap_or(0)].chars().count();
    (
        u16::try_from(column).expect("on screen"),
        u16::try_from(row).expect("on screen"),
    )
}

pub fn click(tui: &mut App, button: MouseButton, (column, row): (u16, u16)) {
    tui.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(button),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    });
}

/// Right-click `name` in the projects list of `tui`, then pick
/// `entry` from its "Move to workspace".
pub fn pick_move(tui: &mut App, name: &str, entry: &str) {
    let rows = screen(tui);
    click(tui, MouseButton::Right, cell_of(&rows, name));
    let rows = screen(tui);
    click(tui, MouseButton::Left, cell_of(&rows, "Move to workspace"));
    let rows = screen(tui);
    click(tui, MouseButton::Left, cell_of(&rows, entry));
}
