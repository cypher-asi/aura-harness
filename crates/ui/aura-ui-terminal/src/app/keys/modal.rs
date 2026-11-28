//! Keys of the modals and prompts: the folder browser, tool approval,
//! login and the model selector.

use crate::app::folder_picker::FolderPicker;
use crate::app::model_picker::PAGE_STEP;
use crate::app::{App, AppState, FormField, NotificationType, PickerTarget};
use crate::{events::UiEvent, terminal::KeyResult};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use std::{io, path::PathBuf};

/// How the user answered a tool call waiting for approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// Allow this call
    Once,
    /// Allow this call and the ones like it, in this project
    Always,
    Deny,
}

impl App {
    /// Handle key while the new-project folder browser is open.
    pub(super) fn handle_folder_picker_key(&mut self, key: KeyEvent) {
        let Some(picker) = self.project_list.folder_picker.as_mut() else {
            self.close_dialog();
            return;
        };
        if picker.new_folder().is_some() {
            if let Err(problem) = new_folder_key(picker, key) {
                self.notifications.current = Some((problem, NotificationType::Warning));
            }
            return;
        }
        if key.code == KeyCode::Esc {
            self.cancel_folder_picker();
            return;
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
    }

    /// Handle key in approval mode: `y` allows the call once, `a` always
    /// (in this project), `n` or Esc denies it. The turn goes on either
    /// way; Ctrl+C cancels it instead.
    pub(super) fn handle_approval_key(&mut self, key: KeyEvent) {
        let always_offered = self
            .pending
            .approvals
            .front()
            .is_some_and(|a| !a.always.is_empty());
        let answer = match key.code {
            KeyCode::Char('y' | 'Y') => Answer::Once,
            KeyCode::Char('a' | 'A') if always_offered => Answer::Always,
            KeyCode::Char('n' | 'N') | KeyCode::Esc => Answer::Deny,
            _ => return,
        };
        self.answer_approval(answer);
    }

    /// Deny the call waiting for approval on top, as `n` does.
    pub(in crate::app) fn deny_approval(&mut self) {
        self.answer_approval(Answer::Deny);
    }

    /// Answer the call waiting for approval on top, then show the next
    /// one waiting, or go back to the turn.
    fn answer_approval(&mut self, answer: Answer) {
        if let Some(approval) = self.pending.approvals.pop_front() {
            self.mark_awaiting_approval(&approval.id, false);
            let (event, said) = match answer {
                Answer::Once => (UiEvent::Approve(approval.id), "Approved"),
                Answer::Always => (UiEvent::ApproveAlways(approval.id), "Always allowed"),
                Answer::Deny => (UiEvent::Deny(approval.id), "Denied"),
            };
            if let Some(tx) = &self.link.event_tx {
                let _ = tx.try_send(event);
            }
            self.status = format!("{said}, continuing...");
        }
        if self.pending.approvals.is_empty() {
            self.chat_state = AppState::Processing;
        } else {
            self.after_approval_closed();
        }
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
        self.chat_state = AppState::Idle;
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

        match self.chat_state {
            AppState::LoginEmail => {
                self.account.login_email = value.trim().to_string();
                self.chat_state = AppState::LoginPassword;
                self.status = "Login — enter password".to_string();
            }
            AppState::LoginPassword => {
                let email = std::mem::take(&mut self.account.login_email);
                self.chat_state = AppState::Processing;
                self.status = "Authenticating...".to_string();
                if let Some(tx) = &self.link.event_tx {
                    let _ = tx.try_send(UiEvent::LoginCredentials {
                        email,
                        password: value,
                    });
                }
            }
            AppState::LinkCode => {
                self.chat_state = AppState::Idle;
                self.status = "Linking...".to_string();
                if let Some(tx) = &self.link.event_tx {
                    let _ = tx.try_send(UiEvent::LinkCode(value.trim().to_string()));
                }
            }
            _ => {}
        }
    }

    /// Handle key while the model selector is open.
    pub(super) fn handle_model_selector_key(&mut self, key: KeyEvent) {
        let models = &self.account.models;
        let picker = &mut self.account.model_picker;
        match key.code {
            KeyCode::Esc => {
                if !picker.clear_filter() {
                    self.close_dialog();
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
            (None, _) if picker.filter().is_empty() => self.close_dialog(),
            (None, _) => {}
        }
    }

    /// A press inside the model selector: one on a model picks it, as
    /// Enter would.
    pub(in crate::app) fn model_selector_click(&mut self, kind: MouseEventKind, row: u16) {
        if kind != MouseEventKind::Down(MouseButton::Left) {
            return;
        }
        if let Some(index) = self.account.model_picker_layout.model_at(row) {
            self.account
                .model_picker
                .select(&self.account.models, index);
            self.pick_in_model_selector();
        }
    }

    /// The model selector list's first row, at most `last`: where the
    /// wheel left it while the cursor is still on row `cursor_row`, else
    /// `follow`, the window that shows the cursor. Drawing reads it with
    /// the list borrowed; [`Self::keep_model_picker_scroll`] keeps it.
    pub(crate) fn model_picker_first(
        &self,
        (cursor_row, last): (usize, usize),
        follow: usize,
    ) -> usize {
        let list = crate::app::wheel_views::WheelList::ModelPicker;
        let first = if self.panels.wheel.holds(list, cursor_row) {
            self.account.model_picker.scroll()
        } else {
            follow
        };
        first.min(last)
    }

    /// Keep `first`, from [`Self::model_picker_first`], as the model
    /// selector list's first row for the wheel's next notch, the cursor
    /// on row `cursor_row`.
    pub(crate) fn keep_model_picker_scroll(&mut self, cursor_row: usize, first: usize) {
        let list = crate::app::wheel_views::WheelList::ModelPicker;
        self.panels.wheel.held(list, cursor_row);
        self.account.model_picker.set_scroll(first);
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
