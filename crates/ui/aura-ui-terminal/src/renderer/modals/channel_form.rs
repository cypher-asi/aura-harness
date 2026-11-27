//! The channel form: the slug, title, description and project fields.
//!
//! ```text
//! ┌ NEW CHANNEL ─────────────────────────┐
//! │                                      │
//! │  › Slug                              │
//! │      zode█                           │
//! │    Title                             │
//! │      zode                            │
//! │    Description                       │
//! │                                      │
//! │    Project                           │
//! │      ‹ Zode ›                        │
//! │      ~/code/zode                     │
//! │                                      │
//! └──────────────────────────────────────┘
//! ```
//!
//! An empty title of a new channel shows the slug it defaults to, muted.
//! A refused slug's rule is the status line, once one is typed. Editing a
//! channel titles the box `EDIT #slug` and shows the slug muted, as it
//! cannot change.

use super::super::hud::{notice_icon, truncate};
use super::super::text::display_width;
use super::with_notice;
use crate::app::channel_form::{ChannelField, ChannelForm, FormMode, SLUG_HINT};
use crate::app::NotificationType;
use crate::components::{caret_line, Modal, ModalArea};
use crate::{App, Theme};
use ratatui::{
    style::Style,
    text::{Line, Span},
    Frame,
};

/// Columns a field's value is indented by.
const INDENT: &str = "    ";

/// Draw the form, if it is open.
pub(super) fn render(frame: &mut Frame, app: &App, theme: &Theme) -> Option<ModalArea> {
    let form = app.channel_form()?;
    let screen = frame.area();
    let modal = Modal::new(title(form), theme);
    let modal = if slug_refused(form) {
        let (icon, color) = notice_icon(NotificationType::Error, theme);
        modal.status(format!("{icon} Slug: {SLUG_HINT}"), color)
    } else {
        with_notice(modal, app, theme)
    };
    let width = modal.body_width(screen);
    let mut lines = Vec::new();
    for field in ChannelField::ALL {
        lines.push(label_line(form, field, theme));
        if field == ChannelField::Project {
            lines.push(project_line(form, theme, width));
            lines.push(project_folder_line(form, theme, width));
        } else {
            lines.push(value_line(form, field, theme, width));
        }
    }
    Some(modal.render(frame.buffer_mut(), screen, lines))
}

/// Whether the slug typed is refused. An empty one is not, yet.
fn slug_refused(form: &ChannelForm) -> bool {
    !form.slug_is_valid() && !form.text(ChannelField::Slug).is_empty()
}

/// The box's title: `NEW CHANNEL`, or `EDIT #slug` while editing.
fn title(form: &ChannelForm) -> String {
    match form.mode() {
        FormMode::Create => "NEW CHANNEL".to_string(),
        FormMode::Edit { slug, .. } => format!("EDIT #{slug}"),
    }
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

/// A text field's text, with the caret in the focused one. An empty
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
    let style = if form.is_focusable(field) {
        Style::default().fg(theme.colors.foreground)
    } else {
        muted
    };
    if form.focus() == field {
        let lead = vec![Span::raw(INDENT)];
        return caret_line(lead, text, form.cursor(field), width, style, theme);
    }
    let room = width.saturating_sub(INDENT.len());
    if field == ChannelField::Title && text.is_empty() && !form.is_edit() {
        let slug = truncate(form.text(ChannelField::Slug), room);
        return Line::from(vec![Span::raw(INDENT), Span::styled(slug, muted)]);
    }
    Line::from(vec![
        Span::raw(INDENT),
        Span::styled(truncate(text, room), style),
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

/// The chosen project's folder, muted, when it is known. The line is
/// blank otherwise, so the box keeps its height as projects are chosen.
fn project_folder_line(form: &ChannelForm, theme: &Theme, width: usize) -> Line<'static> {
    let Some(project) = form.project().filter(|project| !project.folder.is_empty()) else {
        return Line::default();
    };
    let room = width.saturating_sub(INDENT.len());
    Line::from(vec![
        Span::raw(INDENT),
        Span::styled(
            truncate(&project.folder, room),
            Style::default().fg(theme.colors.muted),
        ),
    ])
}
