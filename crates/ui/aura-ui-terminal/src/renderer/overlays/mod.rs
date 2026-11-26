//! Overlay rendering: approval modal, help, and the lines of a record's
//! details.

mod approval;
mod folder_picker;
mod help;
mod model_selector;
mod project_info;
mod record;

use crate::{app::AppState, App, Theme};
use ratatui::Frame;

pub(super) use folder_picker::render_folder_picker;
pub(super) use record::{record_detail_lines, record_status, DETAIL_LABEL_WIDTH};

/// Render overlay elements (modals, help).
pub(super) fn render_overlays(frame: &mut Frame, app: &mut App, theme: &Theme) {
    if let Some(approval) = app.pending_approval() {
        approval::render_approval_modal(frame, approval, app.pending_approval_count(), theme);
    }

    if app.state() == AppState::ShowingHelp {
        help::render_help_overlay(frame, theme);
    }

    if app.state() == AppState::SelectingModel {
        model_selector::render_model_selector(frame, app, theme);
    }

    if let Some(project) = app.project_info() {
        project_info::render_project_info(frame, project, theme);
    }
}
