//! A project's details.

use super::super::text::centered_rect;
use crate::{
    components::{Info, Value},
    Theme,
};
use ratatui::{
    style::Style,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

/// Render a project's details: its name, folder, when it was made, and
/// how many of its agents are listed and archived.
pub(super) fn render_project_info(
    frame: &mut Frame,
    project: &crate::events::ProjectSummary,
    theme: &Theme,
) {
    let area = frame.area();
    let modal_width = 64.min(area.width.saturating_sub(4));
    let mut info = Info::new(usize::from(modal_width.saturating_sub(2)).max(1), theme);
    info.section("FOLDER", None);
    info.text(&project.folder, Value::Plain);
    if !project.folder_exists {
        info.text("The folder is missing", Value::Flag);
    }
    info.section("PROJECT", None);
    info.fact("Created", &project.created, Value::Plain);
    info.fact("Agents", &project.listed_agents.to_string(), Value::Plain);
    info.fact(
        "Archived",
        &project.archived_agents.to_string(),
        Value::Plain,
    );
    info.blank();
    info.note("Press any key to close");
    let lines = info.into_lines();

    let modal_height = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(area.height);
    let modal_area = centered_rect(modal_width, modal_height, area);
    frame.render_widget(Clear, modal_area);
    let block = Block::default()
        .title(format!(" {} ", project.name.to_uppercase()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.muted));
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        modal_area,
    );
}
