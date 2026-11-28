//! The Integrations app: every integration (`docs/integrations.md`) with
//! its status, and Connect, Configure, Test and Disconnect.
//!
//! The list comes from the session (`UiCommand::SetIntegrations`), asked
//! for each time the tab is shown. Actions go back as
//! `UiEvent::Integration`. A key typed for Connect is masked in the input
//! and goes only into that event, whose `Debug` hides it.

use super::super::{App, AppState, NotificationType};
use crate::app::wheel_views::WheelList;
use crate::events::{IntegrationAction, IntegrationState, IntegrationView, UiEvent};
use crate::terminal::KeyResult;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

/// What an action span in the panel does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IntegrationButton {
    /// Ask for a key, or say how to sign in
    Connect,
    /// Ask for a setting's value
    Configure,
    /// Run the health check
    Test,
    /// Remove the stored key
    Disconnect,
}

impl IntegrationButton {
    /// Every button, in the order the panel draws them.
    pub(crate) const ALL: [Self; 4] =
        [Self::Connect, Self::Configure, Self::Test, Self::Disconnect];

    /// Its key and label: `[c] Connect`.
    pub(crate) const fn label(self) -> (char, &'static str) {
        match self {
            Self::Connect => ('c', "Connect"),
            Self::Configure => ('e', "Configure"),
            Self::Test => ('t', "Test"),
            Self::Disconnect => ('d', "Disconnect"),
        }
    }
}

/// What the input is asking for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Asking {
    /// A key for integration `id`
    Key { id: String, prompt: String },
    /// A value for setting `key` of `id`; an empty `key` takes `key=value`
    Setting {
        id: String,
        key: String,
        prompt: String,
    },
}

/// The Integrations app's state.
#[derive(Debug, Default)]
pub(crate) struct IntegrationsState {
    rows: Vec<IntegrationView>,
    selected: usize,
    loaded: bool,
    asking: Option<Asking>,
    /// Where each row's block and each action span were drawn
    row_areas: Vec<(Rect, usize)>,
    button_areas: Vec<(Rect, IntegrationButton)>,
    /// The first line shown; the wheel moves it ([`WheelList::Integrations`])
    pub(in crate::app) scroll: usize,
}

impl App {
    /// The integrations listed, in order.
    #[must_use]
    pub fn integrations(&self) -> &[IntegrationView] {
        &self.integrations.rows
    }

    /// Whether the list has come from the session.
    #[must_use]
    pub const fn integrations_loaded(&self) -> bool {
        self.integrations.loaded
    }

    /// The row the keys act on.
    #[must_use]
    pub const fn selected_integration(&self) -> usize {
        self.integrations.selected
    }

    pub(in crate::app) fn set_integrations(&mut self, rows: Vec<IntegrationView>) {
        self.integrations.selected = self.integrations.selected.min(rows.len().saturating_sub(1));
        self.integrations.rows = rows;
        self.integrations.loaded = true;
    }

    /// Where the renderer drew the rows and the action spans.
    pub(crate) fn set_integration_areas(
        &mut self,
        rows: Vec<(Rect, usize)>,
        buttons: Vec<(Rect, IntegrationButton)>,
    ) {
        self.integrations.row_areas = rows;
        self.integrations.button_areas = buttons;
    }

    /// The first line to show of the list's `total` lines in `visible`
    /// rows, the selected integration's block being lines `block`: where
    /// the wheel left it while the highlight stays, else moved as little
    /// as needed to show the whole block (its top when it is taller than
    /// the view).
    pub(crate) fn place_integrations(
        &mut self,
        block: std::ops::Range<usize>,
        (visible, total): (usize, usize),
    ) -> usize {
        let key = self.integrations.selected;
        let scroll = self.integrations.scroll;
        let placed = if self.panels.wheel.held(WheelList::Integrations, key) {
            scroll.min(total.saturating_sub(visible.max(1)))
        } else {
            // The block's last line in view, then its top, which wins
            // when the block is taller than the view.
            let last = block.end.saturating_sub(1).max(block.start);
            let bottom_first = (last + 1).saturating_sub(visible.max(1));
            scroll.max(bottom_first).min(block.start)
        };
        self.integrations.scroll = placed;
        placed
    }

    /// The tab came to the front: ask for the list again.
    pub(super) fn show_integrations_app(&mut self) {
        self.send_integration("", IntegrationAction::Refresh);
    }

    pub(in crate::app) fn move_integration_highlight(&mut self, delta: isize) {
        let last = self.integrations.rows.len().saturating_sub(1);
        self.integrations.selected = self
            .integrations
            .selected
            .saturating_add_signed(delta)
            .min(last);
    }

    /// The panel's keys: Up / Down pick a row; `c` `e` `t` `d` are its
    /// actions; `r` asks for the list again. Returns whether it took
    /// `key`.
    pub(super) fn handle_integrations_key(&mut self, key: KeyEvent) -> bool {
        let button = match key.code {
            KeyCode::Up => {
                self.move_integration_highlight(-1);
                return true;
            }
            KeyCode::Down => {
                self.move_integration_highlight(1);
                return true;
            }
            KeyCode::Char('r') => {
                self.show_integrations_app();
                return true;
            }
            KeyCode::Char(c) => IntegrationButton::ALL
                .into_iter()
                .find(|button| button.label().0 == c),
            _ => None,
        };
        let Some(button) = button else {
            return false;
        };
        self.press_integration_button(button);
        true
    }

    /// A click in the panel: an action span, else the row it is in.
    pub(super) fn click_integration_row(&mut self, column: u16, row: u16) {
        let inside = |rect: &Rect| super::super::contains(Some(*rect), column, row);
        if let Some(&(_, button)) = self
            .integrations
            .button_areas
            .iter()
            .find(|(r, _)| inside(r))
        {
            self.press_integration_button(button);
        } else if let Some(&(_, index)) =
            self.integrations.row_areas.iter().find(|(r, _)| inside(r))
        {
            self.integrations.selected = index;
        }
    }

    fn press_integration_button(&mut self, button: IntegrationButton) {
        let Some(row) = self
            .integrations
            .rows
            .get(self.integrations.selected)
            .cloned()
        else {
            return;
        };
        match button {
            IntegrationButton::Connect => self.ask_integration_key(&row),
            IntegrationButton::Configure => self.ask_integration_setting(&row),
            IntegrationButton::Test => {
                self.mark_integration_busy(&row.id);
                self.send_integration(&row.id, IntegrationAction::Test);
            }
            IntegrationButton::Disconnect if row.stored => {
                self.send_integration(&row.id, IntegrationAction::Disconnect);
            }
            IntegrationButton::Disconnect => self.notify_integration(match &row.login {
                Some(login) => format!(
                    "{} has no stored key; sign out where you ran {login}",
                    row.name
                ),
                None => format!("{} has no stored key to remove", row.name),
            }),
        }
    }

    fn ask_integration_key(&mut self, row: &IntegrationView) {
        let Some(label) = &row.takes_key else {
            self.notify_integration(match &row.login {
                Some(login) => format!("Run {login} in a terminal, then Test"),
                None => format!("{} needs no sign-in", row.name),
            });
            return;
        };
        let prompt = format!("{} {label}: ", row.name);
        self.ask_integration(Asking::Key {
            id: row.id.clone(),
            prompt,
        });
    }

    fn ask_integration_setting(&mut self, row: &IntegrationView) {
        let asking = match row.config.as_slice() {
            [] => {
                self.notify_integration(format!("{} has no settings", row.name));
                return;
            }
            [field] => Asking::Setting {
                id: row.id.clone(),
                key: field.key.clone(),
                prompt: format!("{} {}: ", row.name, field.label),
            },
            _ => Asking::Setting {
                id: row.id.clone(),
                key: String::new(),
                prompt: format!("{} setting (key=value): ", row.name),
            },
        };
        self.ask_integration(asking);
    }

    fn ask_integration(&mut self, asking: Asking) {
        self.integrations.asking = Some(asking);
        self.input.text.clear();
        self.input.cursor_pos = 0;
        self.chat_state = AppState::IntegrationInput;
    }

    /// A key while the input asks for a key or setting: Enter sends it,
    /// Esc gives up.
    pub(in crate::app) fn handle_integration_input_key(&mut self, key: KeyEvent) -> KeyResult {
        match key.code {
            KeyCode::Esc => {
                self.end_integration_input();
            }
            KeyCode::Enter => self.submit_integration_input(),
            _ => {
                self.edit_input(key);
            }
        }
        KeyResult::continue_running()
    }

    fn end_integration_input(&mut self) -> Option<Asking> {
        self.input.text.clear();
        self.input.cursor_pos = 0;
        self.chat_state = AppState::Idle;
        self.integrations.asking.take()
    }

    fn submit_integration_input(&mut self) {
        let value = std::mem::take(&mut self.input.text);
        match self.end_integration_input() {
            Some(Asking::Key { id, .. }) if !value.trim().is_empty() => {
                self.mark_integration_busy(&id);
                self.send_integration(&id, IntegrationAction::Connect { secret: value });
            }
            Some(Asking::Setting { id, key, .. }) => {
                let (key, value) = if key.is_empty() {
                    let (key, value) = value.split_once('=').unwrap_or((value.as_str(), ""));
                    (key.trim().to_string(), value.trim().to_string())
                } else {
                    (key, value.trim().to_string())
                };
                self.send_integration(&id, IntegrationAction::Configure { key, value });
            }
            _ => {}
        }
    }

    /// The input's prompt while it asks for a key or setting.
    #[must_use]
    pub fn integration_prompt(&self) -> &str {
        match &self.integrations.asking {
            Some(Asking::Key { prompt, .. } | Asking::Setting { prompt, .. }) => prompt,
            None => "",
        }
    }

    /// Whether the input holds a key, drawn masked.
    #[must_use]
    pub const fn integration_input_is_secret(&self) -> bool {
        matches!(self.integrations.asking, Some(Asking::Key { .. }))
    }

    fn mark_integration_busy(&mut self, id: &str) {
        if let Some(row) = self.integrations.rows.iter_mut().find(|row| row.id == id) {
            row.state = IntegrationState::Busy;
        }
    }

    fn notify_integration(&mut self, text: String) {
        self.notifications.current = Some((text, NotificationType::Warning));
    }

    fn send_integration(&mut self, id: &str, action: IntegrationAction) {
        self.try_send_event(UiEvent::Integration {
            id: id.to_string(),
            action,
        });
    }
}
