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
/// Columns between the chat-aligned input edge and the typing column.
const TYPING_INDENT: u16 = 2;

/// Render input line with the current model and status on the right.
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
        && usable_width.saturating_sub(status_len + 2 + left_offset + model_reserved)
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

    let (prompt_str, display_input) = match app.state() {
        AppState::LoginEmail => ("Email: ", input.to_string()),
        AppState::LoginPassword => ("Password: ", "•".repeat(input.len())),
        _ => (app.shell_prompt(), input.to_string()),
    };
    let prompt_width = u16::try_from(display_width(prompt_str)).unwrap_or(u16::MAX);

    // The prompt sits in the gutter left of the chat-aligned typing column,
    // and only pushes typing right when the gutter is too narrow for it.
    let chat_padding: u16 = if left_offset > 0 { 3 } else { EDGE_INSET };
    let typing_x = (area.x + left_offset + chat_padding + TYPING_INDENT)
        .max(area.x + EDGE_INSET + prompt_width);
    let prompt_area = Rect {
        x: typing_x - prompt_width,
        y: area.y,
        width: prompt_width.min(input_right.saturating_sub(typing_x - prompt_width)),
        height: 1,
    };
    let input_area = Rect {
        x: typing_x,
        y: area.y,
        width: input_right.saturating_sub(typing_x),
        height: 1,
    };

    frame.render_widget(
        Paragraph::new(Span::styled(prompt_str, Style::default().fg(prompt_color))),
        prompt_area,
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            display_input,
            Style::default().fg(theme.colors.muted),
        )),
        input_area,
    );

    if !is_thinking {
        let cursor_x = typing_x.saturating_add(u16::try_from(cursor_pos).unwrap_or(u16::MAX));
        frame.set_cursor_position((cursor_x, area.y));
    }
}
