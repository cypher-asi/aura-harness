//! The channel form: a centered box with the slug, title, description and
//! project fields, the slug's inline hint, and the keys that drive it,
//! drawn as the folder browser is.
//!
//! ```text
//!  New channel
//! › Slug
//!     zode█
//!
//!   Title
//!     zode
//!   Description
//!
//!   Project
//!     ‹ Zode ›
//!     ~/code/zode
//!
//! Tab/↑↓ field  ←→ choose project
//! Enter next, submits on Project  Ctrl+Enter submit
//! Esc cancel  Title defaults to the slug
//! ```
//!
//! Editing a channel titles the box ` Edit #slug ` and shows the slug
//! muted, as it cannot change.

use super::hud::truncate;
use super::text::{centered_rect, display_width};
use crate::app::channel_form::{ChannelField, ChannelForm, FormMode, SLUG_HINT};
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
/// hint, the project's folder, a blank line and three lines of keys.
const INNER_HEIGHT: u16 = 14;
/// What marks the cursor in the focused field.
const CURSOR: &str = "█";
/// Columns a field's value is indented by.
const INDENT: &str = "    ";
/// What the form says under "None".
pub(crate) const NO_PROJECT_HINT: &str = "None — agents work in the channel's own folder";

/// Render the form, if it is open.
pub(super) fn render_channel_form(frame: &mut Frame, app: &App, theme: &Theme) {
    if app.state() != AppState::ChannelForm {
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

    let mut lines = Vec::with_capacity(usize::from(INNER_HEIGHT));
    for field in ChannelField::ALL {
        lines.push(label_line(form, field, theme));
        match field {
            ChannelField::Project => {
                lines.push(project_line(form, theme, inner_width));
                lines.push(project_detail_line(form, theme, inner_width));
            }
            _ => lines.push(value_line(form, field, theme, inner_width)),
        }
        if field == ChannelField::Slug {
            lines.push(slug_hint_line(form, theme));
        }
    }
    lines.push(Line::from(""));
    lines.extend(key_lines(form, theme, inner_width));

    frame.render_widget(Clear, modal_area);
    let block = Block::default()
        .title(title(form))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.muted));
    frame.render_widget(Paragraph::new(lines).block(block), modal_area);
}

/// The box's title: ` New channel `, or ` Edit #slug ` while editing.
fn title(form: &ChannelForm) -> String {
    match form.mode() {
        FormMode::Create => " New channel ".to_string(),
        FormMode::Edit { slug, .. } => format!(" Edit #{slug} "),
    }
}

/// The keys that drive the form, each line cut to `width`.
fn key_lines(form: &ChannelForm, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let muted = Style::default().fg(theme.colors.muted);
    let title_rule = if form.is_edit() {
        "Esc cancel  An empty title keeps the current one"
    } else {
        "Esc cancel  Title defaults to the slug"
    };
    [
        "Tab/↑↓ field  ←→ choose project",
        "Enter next, submits on Project  Ctrl+Enter submit",
        title_rule,
    ]
    .into_iter()
    .map(|text| Line::from(Span::styled(truncate(text, width), muted)))
    .collect()
}

/// A field's label, lit while the field has the cursor, muted when it
/// cannot take it.
fn label_line(form: &ChannelForm, field: ChannelField, theme: &Theme) -> Line<'static> {
    let focused = form.focus() == field;
    let marker = if focused { "› " } else { "  " };
    let color = if focused {
        theme.colors.primary
    } else if form.is_focusable(field) {
        theme.colors.secondary
    } else {
        theme.colors.muted
    };
    Line::from(vec![
        Span::styled(marker, Style::default().fg(theme.colors.primary)),
        Span::styled(field.label(), Style::default().fg(color)),
    ])
}

/// A text field's text, with the cursor in the focused one. An empty
/// title of a new channel shows the slug it will default to, muted, and
/// the slug of a channel being edited is muted, as it cannot change.
fn value_line(
    form: &ChannelForm,
    field: ChannelField,
    theme: &Theme,
    width: usize,
) -> Line<'static> {
    let text = form.text(field);
    let muted = Style::default().fg(theme.colors.muted);
    let text_style = if form.is_focusable(field) {
        Style::default().fg(theme.colors.foreground)
    } else {
        muted
    };
    let indent = Span::raw(INDENT);
    let room = width.saturating_sub(INDENT.len());
    if form.focus() != field {
        if field == ChannelField::Title && text.is_empty() && !form.is_edit() {
            let slug = truncate(form.text(ChannelField::Slug), room);
            return Line::from(vec![indent, Span::styled(slug, muted)]);
        }
        return Line::from(vec![indent, Span::styled(truncate(text, room), text_style)]);
    }
    let cursor = form.cursor(field).min(text.len());
    let (before, after) = text.split_at(cursor);
    // Keep the cursor on screen: drop text from the left while the line
    // is too wide.
    let room = room.saturating_sub(display_width(CURSOR));
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

/// The chosen project, `‹ name ›`, or `‹ None ›`; the arrows are lit
/// while the project has the cursor.
fn project_line(form: &ChannelForm, theme: &Theme, width: usize) -> Line<'static> {
    let name = form
        .project()
        .map_or("None", |project| project.name.as_str());
    let arrow_color = if form.focus() == ChannelField::Project {
        theme.colors.primary
    } else {
        theme.colors.muted
    };
    let arrow = Style::default().fg(arrow_color);
    let room = width.saturating_sub(INDENT.len() + display_width("‹  ›"));
    Line::from(vec![
        Span::raw(INDENT),
        Span::styled("‹ ", arrow),
        Span::styled(
            truncate(name, room),
            Style::default().fg(theme.colors.foreground),
        ),
        Span::styled(" ›", arrow),
    ])
}

/// Under the chosen project, muted: its folder when known (else
/// nothing), or, for "None", where the agents work instead.
fn project_detail_line(form: &ChannelForm, theme: &Theme, width: usize) -> Line<'static> {
    let detail = match form.project() {
        Some(project) if project.folder.is_empty() => return Line::from(""),
        Some(project) => project.folder.as_str(),
        None => NO_PROJECT_HINT,
    };
    let room = width.saturating_sub(INDENT.len());
    Line::from(vec![
        Span::raw(INDENT),
        Span::styled(
            truncate(detail, room),
            Style::default().fg(theme.colors.muted),
        ),
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
        Span::raw(INDENT),
        Span::styled(SLUG_HINT, Style::default().fg(color)),
    ])
}
