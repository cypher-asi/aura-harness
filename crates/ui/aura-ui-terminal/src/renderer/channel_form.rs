//! The new-channel form: a centered box with the slug, title and
//! description fields, the slug's inline hint, and the keys that drive
//! it, drawn as the folder browser is.

use super::text::{centered_rect, display_width};
use crate::app::channel_form::{ChannelField, ChannelForm, SLUG_HINT};
use crate::app::AppState;
use crate::{App, Theme};
use ratatui::{
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Widest the box gets, border included.
const WIDTH: u16 = 64;
/// Rows inside the border: a label and a value per field, the slug's
/// hint, a blank line and two lines of keys.
const INNER_HEIGHT: u16 = 10;
/// What marks the cursor in the focused field.
const CURSOR: &str = "█";

/// Render the form, if it is open.
pub(super) fn render_channel_form(frame: &mut Frame, app: &App, theme: &Theme) {
    if app.state() != AppState::NewChannel {
        return;
    }
    let Some(form) = app.channel_form() else {
        return;
    };
    let area = frame.area();
    let width = WIDTH.min(area.width.saturating_sub(4));
    let height = (INNER_HEIGHT + 2).min(area.height.saturating_sub(2));
    let modal_area = centered_rect(width, height, area);
    let inner_width = usize::from(width.saturating_sub(2));

    let muted = Style::default().fg(theme.colors.muted);
    let mut lines = Vec::with_capacity(usize::from(INNER_HEIGHT));
    for field in ChannelField::ALL {
        lines.push(label_line(form, field, theme));
        lines.push(value_line(form, field, theme, inner_width));
        if field == ChannelField::Slug {
            lines.push(slug_hint_line(form, theme));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Tab/↑↓ field  Enter next, submits on Description",
        muted,
    )));
    lines.push(Line::from(Span::styled(
        "Ctrl+Enter submit  Esc cancel  Title defaults to the slug",
        muted,
    )));

    frame.render_widget(Clear, modal_area);
    let block = Block::default()
        .title(" New channel ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.muted));
    frame.render_widget(Paragraph::new(lines).block(block), modal_area);
}

/// A field's label, lit while the field has the cursor.
fn label_line(form: &ChannelForm, field: ChannelField, theme: &Theme) -> Line<'static> {
    let focused = form.focus() == field;
    let marker = if focused { "› " } else { "  " };
    let color = if focused {
        theme.colors.primary
    } else {
        theme.colors.secondary
    };
    Line::from(vec![
        Span::styled(marker, Style::default().fg(theme.colors.primary)),
        Span::styled(field.label(), Style::default().fg(color)),
    ])
}

/// A field's text, with the cursor in the focused one. An empty title
/// shows the slug it will default to, muted.
fn value_line(
    form: &ChannelForm,
    field: ChannelField,
    theme: &Theme,
    width: usize,
) -> Line<'static> {
    let text = form.text(field);
    let text_style = Style::default().fg(theme.colors.foreground);
    let indent = Span::raw("    ");
    if form.focus() != field {
        if field == ChannelField::Title && text.is_empty() {
            let slug = form.text(ChannelField::Slug).to_string();
            return Line::from(vec![
                indent,
                Span::styled(slug, Style::default().fg(theme.colors.muted)),
            ]);
        }
        return Line::from(vec![indent, Span::styled(text.to_string(), text_style)]);
    }
    let cursor = form.cursor(field).min(text.len());
    let (before, after) = text.split_at(cursor);
    // Keep the cursor on screen: drop text from the left while the line
    // is too wide.
    let room = width.saturating_sub(4 + display_width(CURSOR));
    let mut before = before;
    while display_width(before) > room {
        let mut chars = before.chars();
        chars.next();
        before = chars.as_str();
    }
    Line::from(vec![
        indent,
        Span::styled(before.to_string(), text_style),
        Span::styled(CURSOR, Style::default().fg(theme.colors.primary)),
        Span::styled(after.to_string(), text_style),
    ])
}

/// What the slug must be: red once a refused slug is typed, muted while
/// it is empty, nothing once it is good.
fn slug_hint_line(form: &ChannelForm, theme: &Theme) -> Line<'static> {
    if form.slug_is_valid() {
        return Line::from("");
    }
    let color = if form.text(ChannelField::Slug).is_empty() {
        theme.colors.muted
    } else {
        theme.colors.error
    };
    Line::from(vec![
        Span::raw("    "),
        Span::styled(SLUG_HINT, Style::default().fg(color)),
    ])
}
