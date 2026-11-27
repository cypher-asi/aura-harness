//! The dark screen behind a dialog.

use crate::themes::Theme;
use ratatui::{buffer::Buffer, layout::Rect};

/// Darken all of `area`. Every cell is emptied, its text, colors and
/// modifiers gone, and takes the theme's backdrop color, so nothing drawn
/// before shows through, not even the cells kept blank for an image the
/// terminal draws.
pub fn backdrop(buf: &mut Buffer, area: Rect, theme: &Theme) {
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.reset();
                cell.set_bg(theme.colors.backdrop);
            }
        }
    }
}
