//! Input line rendering.

use super::text::display_width;
use crate::{app::AppState, App, Theme};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Render input line with status on the right.
/// `left_offset` shifts the input area to align with the chat panel when swarm is visible.
pub(super) fn render_input(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    theme: &Theme,
    left_offset: u16,
) {
    let input = app.input();
    let cursor_pos = app.cursor_pos();

    let prompt_color = theme.colors.primary;

    let status = app.status();
    let is_ready = status == "Ready";
    let is_thinking = status.contains("Thinking");

    let status_style = if is_ready {
        Style::default().fg(theme.colors.primary)
    } else if is_thinking {
        Style::default().fg(theme.colors.muted)
    } else {
        Style::default().fg(theme.colors.warning)
    };

    let status_text = if is_ready {
        format!(" ●  {status}")
    } else if is_thinking {
        format!(" {}  {status}", app.spinner_char())
    } else {
        format!(" ◐  {status}")
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "status text is always short"
    )]
    let status_len = display_width(&status_text) as u16;

    let input_width = area.width.saturating_sub(status_len + 2 + left_offset);

    let status_area = Rect {
        x: area.x + area.width.saturating_sub(status_len),
        y: area.y,
        width: status_len,
        height: 1,
    };
    let status_line = Line::from(Span::styled(&status_text, status_style));
    frame.render_widget(Paragraph::new(status_line), status_area);

    let chat_padding: u16 = if left_offset > 0 { 3 } else { 0 };
    let input_area = Rect {
        x: area.x + left_offset + chat_padding,
        y: area.y,
        width: input_width.saturating_sub(chat_padding),
        height: 1,
    };

    let (prompt_str, display_input) = match app.state() {
        AppState::LoginEmail => ("Email: ", input.to_string()),
        AppState::LoginPassword => ("Password: ", "•".repeat(input.len())),
        _ => ("> ", input.to_string()),
    };

    let content = Line::from(vec![
        Span::styled(prompt_str, Style::default().fg(prompt_color)),
        Span::styled(display_input, Style::default().fg(theme.colors.muted)),
    ]);

    frame.render_widget(Paragraph::new(content), input_area);

    if !is_thinking {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "cursor position fits in terminal width"
        )]
        let prompt_len = prompt_str.len() as u16;
        let cursor_x = area.x
            + left_offset
            + chat_padding
            + prompt_len
            + u16::try_from(cursor_pos).unwrap_or(u16::MAX);
        let cursor_y = area.y;
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}
