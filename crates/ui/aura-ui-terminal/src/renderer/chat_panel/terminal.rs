//! The terminal inline in the Chat panel: the commands run this session
//! and their output from the body's top, then the prompt where the next
//! command is typed, right under the last line, as in a terminal. Once
//! the output fills the body it scrolls up and the prompt keeps the
//! bottom rows; no input box sits under the panel.
//!
//! ```text
//! │  [u@h d]$ echo hi                    │
//! │  hi                                  │
//! │  [u@h d]$ ls█                        │
//! │                                      │
//! └──────────────────────────────────────┘
//! ```

use super::live_tail::LiveTail;
use super::{clear_chat_rows, render_chat_lines, render_chat_scroll};
use crate::renderer::input;
use crate::{App, Theme};
use ratatui::{layout::Rect, Frame};

/// Draw the terminal into the Chat panel `area`, whose body is `body`
/// and whose text goes in `padded`: the output, then the prompt.
pub(super) fn render_terminal(
    frame: &mut Frame,
    (area, body, padded): (Rect, Rect, Rect),
    app: &mut App,
    theme: &Theme,
    is_focused: bool,
) {
    // The output keeps at least a row, the prompt the rest it wants.
    let most = padded.height.saturating_sub(1).max(1);
    let prompt_rows = input::input_rows(area, app, most).min(padded.height);
    let output = Rect {
        height: padded.height - prompt_rows,
        ..padded
    };
    let output_body = Rect {
        height: body.height.saturating_sub(prompt_rows),
        ..body
    };
    app.set_chat_text_area(output);
    let shown = if app.pane_is_empty() {
        clear_chat_rows(app, output);
        0
    } else {
        let rows = render_chat_lines(
            frame,
            (output_body, output),
            app,
            theme,
            &LiveTail::default(),
        );
        render_chat_scroll(frame, app, (area, output), (&rows, is_focused), theme);
        shown_rows(
            rows.total.saturating_sub(rows.padding),
            rows.first,
            output.height,
        )
    };
    let prompt = Rect {
        x: area.x,
        y: output.y + shown,
        width: area.width,
        height: prompt_rows,
    };
    input::show_input(frame, prompt, app, theme, true);
}

/// Rows of output on screen: the `content` rows from the `first` shown,
/// at most the `height` the output has.
fn shown_rows(content: usize, first: usize, height: u16) -> u16 {
    u16::try_from(content.saturating_sub(first))
        .unwrap_or(u16::MAX)
        .min(height)
}

#[cfg(test)]
mod tests {
    use super::shown_rows;

    #[test]
    fn the_prompt_follows_short_output_and_keeps_the_bottom_of_long_output() {
        assert_eq!(shown_rows(0, 0, 10), 0, "an empty terminal: at the top");
        assert_eq!(shown_rows(3, 0, 10), 3, "under the third line");
        assert_eq!(shown_rows(40, 30, 10), 10, "scrolled to the end");
        assert_eq!(shown_rows(40, 5, 10), 10, "scrolled back");
    }
}
