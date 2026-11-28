//! The modal on top: the approval waiting for an answer, or the dialog
//! open under it. Only the one on top is drawn, through a [`Modal`],
//! which empties the whole screen around it so nothing underneath shows.
//!
//! The HUD's notice is under the backdrop too, so a notification raised
//! while a modal is open, such as a folder that cannot be opened, is the
//! modal's status line instead.

mod approval;
mod channel_form;
mod context;
mod folder_picker;
mod help;
mod model_selector;
mod project_info;

pub(super) use channel_form::placement_name;

use super::hud::notice_icon;
use crate::app::{Dialog, ModalKind};
use crate::components::Modal;
use crate::{App, Theme};
use ratatui::Frame;

/// Draw the modal on top, if any, and record where it went for the
/// pointer.
pub(super) fn render_modal_layer(frame: &mut Frame, app: &mut App, theme: &Theme) {
    let area = match app.modal() {
        None => None,
        Some(ModalKind::Approval) => approval::render(frame, app, theme),
        Some(ModalKind::Dialog(Dialog::Help)) => Some(help::render(frame, app, theme)),
        Some(ModalKind::Dialog(Dialog::ModelSelector)) => {
            Some(model_selector::render(frame, app, theme))
        }
        Some(ModalKind::Dialog(Dialog::FolderPicker)) => folder_picker::render(frame, app, theme),
        Some(ModalKind::Dialog(Dialog::ProjectInfo)) => project_info::render(frame, app, theme),
        Some(ModalKind::Dialog(Dialog::ChannelForm)) => channel_form::render(frame, app, theme),
        Some(ModalKind::Dialog(Dialog::Context)) => Some(context::render(frame, app, theme)),
    };
    app.set_modal_area(area);
}

/// `modal` with the notification on screen, if any, as its status.
fn with_notice<'t>(modal: Modal<'t>, app: &App, theme: &Theme) -> Modal<'t> {
    match app.notification() {
        Some((text, kind)) => {
            let (icon, color) = notice_icon(*kind, theme);
            modal.status(format!("{icon} {text}"), color)
        }
        None => modal,
    }
}
