//! A project's details.

use super::with_notice;
use crate::components::{Info, Modal, ModalArea, Value};
use crate::{App, Theme};
use ratatui::Frame;

/// Draw the details of the project shown: its folder, when it was made,
/// and how many of its agents are listed and archived.
pub(super) fn render(frame: &mut Frame, app: &App, theme: &Theme) -> Option<ModalArea> {
    let project = app.project_info()?;
    let screen = frame.area();
    let modal = with_notice(Modal::new(project.name.to_uppercase(), theme), app, theme);
    let mut info = Info::new(modal.body_width(screen).max(1), theme);
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
    Some(modal.render(frame.buffer_mut(), screen, info.into_lines()))
}
