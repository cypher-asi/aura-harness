//! The dialog open over the screen, and the approval that stands over it.
//!
//! What the chat is doing (idle, at work, waiting on an approval, or
//! asking for a sign-in on the input line) is kept apart from the dialog,
//! in `App::chat_state`. A turn that starts, ends or asks for approval
//! leaves an open dialog alone, and closing a dialog goes back to whatever
//! the chat is doing. An approval that arrives while a dialog is open
//! stands over it; once it is answered, the dialog is back.
//!
//! What stands on top, the [`ModalKind`], takes keys, pastes and the
//! pointer before anything under it. Esc backs out of it, and so does a
//! press off its box: the approval is denied, the channel form cancelled,
//! the folder browser cancelled (back to the channel form, when the form
//! opened it), any other dialog closed.

use super::{contains, App, AppState};
use crate::components::ModalArea;
use crossterm::event::{KeyCode, KeyEvent, MouseEvent, MouseEventKind};

/// Lines the help dialog scrolls by for a notch of the wheel.
const WHEEL_LINES: usize = 3;

/// Lines it scrolls by for PgUp and PgDn.
const PAGE_LINES: usize = 10;

/// A dialog over the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialog {
    /// The commands and keys, `/help`
    Help,
    /// Picking a model, for the session or a benchmark row
    ModelSelector,
    /// Picking a new project's folder
    FolderPicker,
    /// A project's details
    ProjectInfo,
    /// Making or editing a channel
    ChannelForm,
}

impl Dialog {
    /// What [`App::state`] says while this dialog is on top.
    const fn state(self) -> AppState {
        match self {
            Self::Help => AppState::ShowingHelp,
            Self::ModelSelector => AppState::SelectingModel,
            Self::FolderPicker => AppState::SelectingFolder,
            Self::ProjectInfo => AppState::ShowingProjectInfo,
            Self::ChannelForm => AppState::ChannelForm,
        }
    }
}

/// What stands over the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalKind {
    /// A tool call waiting for the user's answer, over any dialog
    Approval,
    Dialog(Dialog),
}

/// The dialog open over the screen, the modal on top as last drawn, and
/// the help dialog's scroll.
#[derive(Debug, Default)]
pub(crate) struct ModalLayer {
    /// The dialog open, under an approval or not
    dialog: Option<Dialog>,
    /// Where the modal on top was drawn, for the pointer
    area: Option<ModalArea>,
    /// The first line of the help dialog shown
    help_scroll: usize,
}

impl App {
    /// What the app is doing: the dialog on top's state, unless an
    /// approval stands over it, else the chat's.
    #[must_use]
    pub const fn state(&self) -> AppState {
        match (self.chat_state, self.modals.dialog) {
            (AppState::AwaitingApproval, _) | (_, None) => self.chat_state,
            (_, Some(dialog)) => dialog.state(),
        }
    }

    /// What stands over the screen, taking keys and the pointer first.
    #[must_use]
    pub const fn modal(&self) -> Option<ModalKind> {
        if matches!(self.chat_state, AppState::AwaitingApproval) {
            return Some(ModalKind::Approval);
        }
        match self.modals.dialog {
            Some(dialog) => Some(ModalKind::Dialog(dialog)),
            None => None,
        }
    }

    /// The open dialog, under an approval or not.
    #[must_use]
    pub const fn dialog(&self) -> Option<Dialog> {
        self.modals.dialog
    }

    /// Record where the modal on top was drawn this frame, `None` when
    /// none was.
    pub(crate) const fn set_modal_area(&mut self, area: Option<ModalArea>) {
        self.modals.area = area;
    }

    /// Where the modal on top was last drawn.
    #[cfg(test)]
    pub(crate) const fn modal_area(&self) -> Option<ModalArea> {
        self.modals.area
    }

    /// The first line of the help dialog to show.
    pub(crate) const fn help_scroll(&self) -> usize {
        self.modals.help_scroll
    }

    /// Keep the help dialog's first line where it was drawn from, inside
    /// the help.
    pub(crate) const fn set_help_scroll(&mut self, first: usize) {
        self.modals.help_scroll = first;
    }

    /// Show `dialog` in place of the one open, keeping what that one was
    /// editing. The channel form uses this so the folder browser can sit
    /// over it: [`Self::open_dialog`] would drop the form.
    pub(super) fn replace_dialog(&mut self, dialog: Dialog) {
        self.modals.dialog = Some(dialog);
        self.modals.help_scroll = 0;
        self.modals.area = None;
        self.release_pointer();
        self.clear_selection();
    }

    /// Open `dialog` over the screen, in place of another one open. What
    /// the pointer held underneath is let go, and the selection cleared.
    pub(super) fn open_dialog(&mut self, dialog: Dialog) {
        if self.modals.dialog != Some(dialog) {
            self.close_dialog();
        }
        self.modals.dialog = Some(dialog);
        self.modals.help_scroll = 0;
        self.release_pointer();
        self.clear_selection();
    }

    /// Close the open dialog, dropping what it was editing. The chat is
    /// back as it is now, or the approval that stood over the dialog.
    pub(super) fn close_dialog(&mut self) {
        match self.modals.dialog.take() {
            Some(Dialog::FolderPicker) => self.project_list.folder_picker = None,
            Some(Dialog::ProjectInfo) => self.project_list.project_info = None,
            Some(Dialog::ChannelForm) => self.channel_form = None,
            Some(Dialog::Help | Dialog::ModelSelector) | None => {}
        }
        self.modals.area = None;
    }

    /// Let go of what the pointer held under a modal: the highlight under
    /// it, the context menu and any press.
    pub(super) fn release_pointer(&mut self) {
        self.hover = None;
        self.context_menu = None;
        self.project_list.project_press = None;
        self.select.scrollbar_grab = None;
        self.drop_queue_press();
        self.drop_todo_drawer_hover();
    }

    /// Esc, or a press off the modal on top: the approval is denied, the
    /// folder browser cancelled (back to the channel form, when the form
    /// opened it), any other dialog closed.
    pub(super) fn dismiss_modal(&mut self) {
        match self.modal() {
            Some(ModalKind::Approval) => self.deny_approval(),
            Some(ModalKind::Dialog(Dialog::FolderPicker)) => self.cancel_folder_picker(),
            Some(ModalKind::Dialog(_)) => self.close_dialog(),
            None => {}
        }
    }

    /// A key while the help dialog is on top: the arrows, the page keys,
    /// Home and End scroll it, and any other key closes it.
    pub(super) fn help_key(&mut self, key: KeyEvent) {
        let scroll = &mut self.modals.help_scroll;
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => *scroll = scroll.saturating_add(1),
            KeyCode::PageUp => *scroll = scroll.saturating_sub(PAGE_LINES),
            KeyCode::PageDown => *scroll = scroll.saturating_add(PAGE_LINES),
            KeyCode::Home => *scroll = 0,
            KeyCode::End => *scroll = usize::MAX,
            _ => self.close_dialog(),
        }
    }

    /// A pointer event while `modal` is on top. A press off its box backs
    /// out of it as Esc does; inside it, a press picks a model and the
    /// wheel moves through a list.
    pub(super) fn modal_mouse(&mut self, modal: ModalKind, mouse: MouseEvent) {
        self.release_pointer();
        let inside = self
            .modals
            .area
            .is_some_and(|area| contains(Some(area.frame), mouse.column, mouse.row));
        if !inside {
            if matches!(mouse.kind, MouseEventKind::Down(_)) {
                self.dismiss_modal();
            }
            return;
        }
        let ModalKind::Dialog(dialog) = modal else {
            return;
        };
        let down = match mouse.kind {
            MouseEventKind::ScrollUp => false,
            MouseEventKind::ScrollDown => true,
            kind => {
                if dialog == Dialog::ModelSelector {
                    self.model_selector_click(kind, mouse.row);
                }
                return;
            }
        };
        match dialog {
            Dialog::Help => {
                let scroll = &mut self.modals.help_scroll;
                *scroll = if down {
                    scroll.saturating_add(WHEEL_LINES)
                } else {
                    scroll.saturating_sub(WHEEL_LINES)
                };
            }
            Dialog::ModelSelector => self.move_model_selector(down),
            Dialog::FolderPicker => {
                if let Some(picker) = self.project_list.folder_picker.as_mut() {
                    picker.move_by(if down { 1 } else { -1 });
                }
            }
            Dialog::ProjectInfo | Dialog::ChannelForm => {}
        }
    }
}
