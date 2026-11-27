//! The empty screen behind a dialog.

use crate::themes::Theme;
use ratatui::{buffer::Buffer, layout::Rect};

/// Empty all of `area`. Every cell loses its text, colors and modifiers
/// and takes the theme's background color, so nothing drawn before shows
/// through, not even the cells kept blank for an image the terminal draws.
/// The color is set rather than left `Reset`: the desktop draws its bars
/// over blank cells left `Reset`.
pub fn backdrop(buf: &mut Buffer, area: Rect, theme: &Theme) {
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.reset();
                cell.set_bg(theme.colors.background);
            }
        }
    }
}
