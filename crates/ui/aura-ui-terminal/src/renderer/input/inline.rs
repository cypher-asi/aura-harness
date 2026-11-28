//! Drawing the input where it goes this frame: in its box under the Chat
//! panel, or inline in the terminal, where the prompt follows the output
//! as a terminal's does.

use super::{render_input, render_text, InputDrawn, MIN_WIDTH};
use crate::{App, Theme};
use ratatui::{layout::Rect, Frame};

/// Draw the input into `area` and record where it went for the pointer:
/// in a box under the Chat panel, or `inline` in the panel's body, where
/// the terminal's prompt follows its output (see [`render_inline`]).
pub(in crate::renderer) fn show_input(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    theme: &Theme,
    inline: bool,
) {
    app.set_input_area(area);
    let drawn = if inline {
        render_inline(frame, area, app, theme)
    } else {
        render_input(frame, area, app, theme)
    };
    app.set_input_stop_area(drawn.stop);
    app.set_input_wrap_width(drawn.wrap_width);
    app.set_input_rows(drawn.rows);
}

/// Render the input without a box into `area`, rows of the Chat panel's
/// body spanning the panel's columns, borders included, so the prompt
/// lines up with the text above it. The panel's borders are drawn.
fn render_inline(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) -> InputDrawn {
    if area.height == 0 || area.width < MIN_WIDTH {
        return InputDrawn::default();
    }
    let inner = Rect {
        x: area.x + 1,
        width: area.width - 2,
        ..area
    };
    render_text(frame, area, inner, app, theme)
}
