//! Input line rendering.

use super::{text::display_width, EDGE_INSET};
use crate::{app::AppState, App, Theme};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Columns between the model indicator and the status.
const MODEL_GAP: u16 = 2;
/// Below this many typing columns the model indicator is hidden.
const MIN_INPUT_WIDTH: u16 = 20;

/// Render input line with the current model and status on the right.
/// `chat_left` is the Chat panel's left border, relative to `area`; typing
/// starts there, with the prompt to its left.
pub(super) fn render_input(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    theme: &Theme,
    chat_left: u16,
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
        format!("● {status}")
    } else if is_thinking {
        format!("{} {status}", app.spinner_char())
    } else {
        format!("◐ {status}")
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "status text is always short"
    )]
    let status_len = display_width(&status_text) as u16;

    let model_spans = super::model_indicator_spans(app, theme);
    let model_len = u16::try_from(super::spans_width(&model_spans)).unwrap_or(u16::MAX);
    let model_reserved = if model_len == 0 {
        0
    } else {
        model_len + MODEL_GAP
    };
    let usable_width = area.width.saturating_sub(EDGE_INSET);
    let show_model = model_len > 0
        && usable_width.saturating_sub(status_len + 2 + chat_left + model_reserved)
            >= MIN_INPUT_WIDTH;
    let left_reserved = if show_model { model_reserved } else { 0 };

    if show_model {
        let model_area = Rect {
            x: area.x + usable_width.saturating_sub(status_len + model_reserved),
            y: area.y,
            width: model_len,
            height: 1,
        };
        frame.render_widget(Paragraph::new(Line::from(model_spans)), model_area);
    }

    let input_right = area.x + usable_width.saturating_sub(status_len + 2 + left_reserved);

    let status_area = Rect {
        x: area.x + usable_width.saturating_sub(status_len),
        y: area.y,
        width: status_len,
        height: 1,
    };
    let status_line = Line::from(Span::styled(&status_text, status_style));
    frame.render_widget(Paragraph::new(status_line), status_area);

    let before_cursor = input.get(..cursor_pos).unwrap_or(input);
    let (prompt_str, display_input, cursor_col) = match app.state() {
        AppState::LoginEmail => ("Email: ", input.to_string(), display_width(before_cursor)),
        AppState::LinkCode => (
            "Code: ",
            "•".repeat(input.chars().count()),
            before_cursor.chars().count(),
        ),
        AppState::LoginPassword => (
            "Password: ",
            "•".repeat(input.chars().count()),
            before_cursor.chars().count(),
        ),
        _ => (
            app.shell_prompt(),
            input.to_string(),
            display_width(before_cursor),
        ),
    };
    let prompt_width = u16::try_from(display_width(prompt_str)).unwrap_or(u16::MAX);

    // Typing starts at the Chat panel's left border, inset one column like
    // the other outer text, with the prompt right-aligned under the Agents
    // column before it. Typing moves right only when the prompt is wider
    // than the space left of the chat.
    let typing_x = (area.x + chat_left + EDGE_INSET).max(area.x + EDGE_INSET + prompt_width);
    let prompt_x = typing_x - prompt_width;
    let prompt_area = Rect {
        x: prompt_x,
        y: area.y,
        width: prompt_width.min(input_right.saturating_sub(prompt_x)),
        height: 1,
    };
    let input_area = Rect {
        x: typing_x,
        y: area.y,
        width: input_right.saturating_sub(typing_x),
        height: 1,
    };

    frame.render_widget(
        Paragraph::new(Line::from(prompt_spans(prompt_str, prompt_color, theme))),
        prompt_area,
    );
    let input_line = if app.input_is_command() {
        Line::from(super::command::command_spans(&display_input, theme))
    } else {
        Line::from(Span::styled(
            display_input.as_str(),
            Style::default().fg(theme.colors.muted),
        ))
    };
    frame.render_widget(Paragraph::new(input_line), input_area);

    if !is_thinking {
        let cursor_x = typing_x.saturating_add(u16::try_from(cursor_col).unwrap_or(u16::MAX));
        frame.set_cursor_position((cursor_x, area.y));
    }
}

/// `[user@host dir]$ ` with the user in the success color and `@host` muted;
/// any other prompt is drawn in `color`.
pub(super) fn prompt_spans<'a>(
    prompt: &'a str,
    color: ratatui::style::Color,
    theme: &Theme,
) -> Vec<Span<'a>> {
    let plain = || vec![Span::styled(prompt, Style::default().fg(color))];
    let Some(body) = prompt.strip_prefix('[') else {
        return plain();
    };
    let Some(at) = body.find('@') else {
        return plain();
    };
    let host_end = body[at..].find(' ').map_or(body.len(), |i| at + i);
    vec![
        Span::styled("[", Style::default().fg(color)),
        Span::styled(&body[..at], Style::default().fg(theme.colors.success)),
        Span::styled(&body[at..host_end], Style::default().fg(theme.colors.muted)),
        Span::styled(&body[host_end..], Style::default().fg(color)),
    ]
}
