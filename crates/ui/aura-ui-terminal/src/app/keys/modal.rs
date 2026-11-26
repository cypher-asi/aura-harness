//! Keys of the modal states: the folder browser, tool approval, login and
//! the model selector.

use crate::app::folder_picker::FolderPicker;
use crate::app::model_picker::PAGE_STEP;
use crate::app::{App, AppState, FormField, NotificationType, PickerTarget};
use crate::{events::UiEvent, terminal::KeyResult};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use std::{io, path::PathBuf};

impl App {
    /// Handle key while the new-project folder browser is open.
    pub(super) fn handle_folder_picker_key(&mut self, key: KeyEvent) -> KeyResult {
        let Some(picker) = self.project_list.folder_picker.as_mut() else {
            self.state = AppState::Idle;
            return KeyResult::continue_running();
        };
        if picker.new_folder().is_some() {
            if let Err(problem) = new_folder_key(picker, key) {
                self.notifications.current = Some((problem, NotificationType::Warning));
            }
            return KeyResult::continue_running();
        }
        if key.code == KeyCode::Esc {
            self.cancel_folder_picker();
            return KeyResult::continue_running();
        }
        match browse_folder_key(picker, key) {
            Ok(Some(folder)) => self.choose_project_folder(folder),
            Ok(None) => {}
            Err(e) => {
                self.notifications.current = Some((
                    format!("Cannot open folder: {e}"),
                    NotificationType::Warning,
                ));
            }
        }
        KeyResult::continue_running()
    }

    /// Handle key in approval mode: `y` allows the call once, `a` always
    /// (in this project), `n` or Esc denies it. The turn goes on either
    /// way; Ctrl+C cancels it instead.
    pub(super) fn handle_approval_key(&mut self, key: KeyEvent) -> KeyResult {
        let always_offered = self
            .pending
            .approvals
            .front()
            .is_some_and(|a| !a.always.is_empty());
        let answer = match key.code {
            KeyCode::Char('y' | 'Y') => "Approved",
            KeyCode::Char('a' | 'A') if always_offered => "Always allowed",
            KeyCode::Char('n' | 'N') | KeyCode::Esc => "Denied",
            _ => return KeyResult::continue_running(),
        };
        if let Some(approval) = self.pending.approvals.pop_front() {
            self.mark_awaiting_approval(&approval.id, false);
            let event = match key.code {
                KeyCode::Char('y' | 'Y') => UiEvent::Approve(approval.id),
                KeyCode::Char('a' | 'A') => UiEvent::ApproveAlways(approval.id),
                _ => UiEvent::Deny(approval.id),
            };
            if let Some(tx) = &self.link.event_tx {
                let _ = tx.try_send(event);
            }
            self.status = format!("{answer}, continuing...");
        }
        if self.pending.approvals.is_empty() {
            self.state = AppState::Processing;
        } else {
            self.after_approval_closed();
        }
        KeyResult::continue_running()
    }

    /// Handle key during login flow (email or password entry).
    pub(super) fn handle_login_key(&mut self, key: KeyEvent) -> KeyResult {
        match key.code {
            KeyCode::Esc => self.cancel_login(),
            KeyCode::Enter => self.submit_login(),
            _ => {
                self.edit_input(key);
            }
        }
        KeyResult::continue_running()
    }

    fn cancel_login(&mut self) {
        self.state = AppState::Idle;
        self.status = "Ready".to_string();
        self.input.text.clear();
        self.input.cursor_pos = 0;
        self.account.login_email.clear();
        self.pending.model = None;
        self.notifications.current =
            Some(("Login cancelled".to_string(), NotificationType::Warning));
    }

    /// Enter in the login flow: take the typed email, password or link
    /// code and go to the next step.
    fn submit_login(&mut self) {
        let value = std::mem::take(&mut self.input.text);
        self.input.cursor_pos = 0;

        if value.trim().is_empty() {
            self.notifications.current =
                Some(("Cannot be empty".to_string(), NotificationType::Warning));
            return;
        }

        match self.state {
            AppState::LoginEmail => {
                self.account.login_email = value.trim().to_string();
                self.state = AppState::LoginPassword;
                self.status = "Login — enter password".to_string();
            }
            AppState::LoginPassword => {
                let email = std::mem::take(&mut self.account.login_email);
                self.state = AppState::Processing;
                self.status = "Authenticating...".to_string();
                if let Some(tx) = &self.link.event_tx {
                    let _ = tx.try_send(UiEvent::LoginCredentials {
                        email,
                        password: value,
                    });
                }
            }
            AppState::LinkCode => {
                self.state = AppState::Idle;
                self.status = "Linking...".to_string();
                if let Some(tx) = &self.link.event_tx {
                    let _ = tx.try_send(UiEvent::LinkCode(value.trim().to_string()));
                }
            }
            _ => {}
        }
    }

    /// Handle key while the model selector is open.
    pub(super) fn handle_model_selector_key(&mut self, key: KeyEvent) -> KeyResult {
        let models = &self.account.models;
        let picker = &mut self.account.model_picker;
        match key.code {
            KeyCode::Esc => {
                if !picker.clear_filter() {
                    self.state = AppState::Idle;
                }
            }
            KeyCode::Up => picker.move_by(models, -1),
            KeyCode::Down => picker.move_by(models, 1),
            KeyCode::PageUp => picker.move_by(models, -PAGE_STEP),
            KeyCode::PageDown => picker.move_by(models, PAGE_STEP),
            KeyCode::Home => picker.home(),
            KeyCode::End => picker.end(models),
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right => {
                picker.switch_tab();
            }
            KeyCode::Backspace => picker.pop(),
            KeyCode::Enter => self.pick_in_model_selector(),
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                picker.push(c);
            }
            _ => {}
        }
        KeyResult::continue_running()
    }

    /// Take the highlighted model of the selector for what it was opened
    /// for: the session's model, or a benchmark row's. With nothing
    /// highlighted and nothing typed it closes.
    fn pick_in_model_selector(&mut self) {
        let picker = &self.account.model_picker;
        let picked = picker.selected(&self.account.models).map(|m| m.id.clone());
        match (picked, picker.target()) {
            (Some(id), PickerTarget::Session) => self.choose_model(&id),
            (Some(id), target) => {
                if let Some(field) = FormField::of_picker(target) {
                    self.pick_bench_model(field, &id);
                }
            }
            (None, _) if picker.filter().is_empty() => self.state = AppState::Idle,
            (None, _) => {}
        }
    }

    /// A mouse event while the model selector is open: a click on a model
    /// picks it, as Enter would, a click outside the selector closes it,
    /// and the wheel moves the highlight.
    pub(in crate::app) fn model_selector_mouse(
        &mut self,
        kind: MouseEventKind,
        column: u16,
        row: u16,
    ) {
        let layout = &self.account.model_picker_layout;
        let inside = crate::app::contains(Some(layout.modal), column, row);
        match kind {
            MouseEventKind::Down(MouseButton::Left) if !inside => self.state = AppState::Idle,
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = layout.model_at(row) {
                    self.account
                        .model_picker
                        .select(&self.account.models, index);
                    self.pick_in_model_selector();
                }
            }
            MouseEventKind::ScrollUp => self.account.model_picker.move_by(&self.account.models, -1),
            MouseEventKind::ScrollDown => {
                self.account.model_picker.move_by(&self.account.models, 1);
            }
            _ => {}
        }
    }
}

/// A key while the folder browser names a new folder.
fn new_folder_key(picker: &mut FolderPicker, key: KeyEvent) -> Result<(), String> {
    match key.code {
        KeyCode::Esc => picker.cancel_new_folder(),
        KeyCode::Enter => return picker.create_new_folder(),
        KeyCode::Backspace => picker.pop_new_folder_char(),
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            picker.push_new_folder_char(c);
        }
        _ => {}
    }
    Ok(())
}

/// A key (other than Esc) while the folder browser lists folders; returns
/// the folder picked, if any.
fn browse_folder_key(picker: &mut FolderPicker, key: KeyEvent) -> io::Result<Option<PathBuf>> {
    match key.code {
        KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => return picker.activate(),
        KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => {
            return picker.go_up().map(|()| None);
        }
        KeyCode::Char(' ') => return Ok(picker.highlighted_folder()),
        KeyCode::Char('~') => return picker.go_home().map(|()| None),
        KeyCode::Up | KeyCode::Char('k') => picker.move_by(-1),
        KeyCode::Down | KeyCode::Char('j') => picker.move_by(1),
        KeyCode::PageUp => picker.move_by(-10),
        KeyCode::PageDown => picker.move_by(10),
        KeyCode::Home => picker.select_first(),
        KeyCode::End => picker.select_last(),
        KeyCode::Char('.') => picker.toggle_hidden(),
        KeyCode::Char('n') => picker.start_new_folder(),
        _ => {}
    }
    Ok(None)
}
