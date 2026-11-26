//! Keys in normal mode (idle or processing): the prompt's menus, global
//! shortcuts, the focused panel's shortcuts, then typing and the panel.

use crate::app::line_edit::is_newline_key;
use crate::app::{AgentsTab, App, ChatTab, PanelFocus};
use crate::{events::UiEvent, terminal::KeyResult};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl App {
    /// Handle key in normal mode.
    pub(super) fn handle_normal_key(&mut self, key: KeyEvent) -> KeyResult {
        if let Some(result) = self.prompt_overlay_key(key) {
            return result;
        }
        if self.global_shortcut_key(key) || self.alt_shortcut_key(key) {
            return KeyResult::continue_running();
        }
        let word_motion = key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        if self.panel_shortcut_key(key, word_motion) {
            return KeyResult::continue_running();
        }
        if let Some(result) = self.typing_key(key, word_motion) {
            return result;
        }
        self.focused_panel_key(key)
    }

    /// Pending confirmations, line breaks, the mention and slash menus,
    /// the queue and the terminal prompt toggle, in that order.
    fn prompt_overlay_key(&mut self, key: KeyEvent) -> Option<KeyResult> {
        if self.handle_pending_delete(key)
            || self.handle_pending_remove_project(key)
            || self.handle_pending_archive_channel(key)
            || self.handle_pending_kill(key)
        {
            return Some(KeyResult::continue_running());
        }
        // Before the menus, which take Enter to pick a row: a line break
        // is never a pick or a send.
        if self.insert_newline_key(key) {
            return Some(KeyResult::continue_running());
        }
        if let Some(result) = self.handle_mention_menu_key(key) {
            return Some(result);
        }
        if let Some(result) = self.handle_slash_menu_key(key) {
            return Some(result);
        }
        if let Some(result) = self.handle_queue_key(key) {
            return Some(result);
        }
        if self.toggle_terminal_mode(key) {
            return Some(KeyResult::continue_running());
        }
        None
    }

    /// Tab, Shift+Tab and the Ctrl shortcuts. Returns whether the key was
    /// used.
    fn global_shortcut_key(&mut self, key: KeyEvent) -> bool {
        if key.code == KeyCode::Tab {
            self.focus = self.next_panel_focus();
            return true;
        }
        if key.code == KeyCode::BackTab {
            if let Some(tx) = &self.link.event_tx {
                let _ = tx.try_send(UiEvent::CycleApproval(self.active_agent_id.clone()));
            }
            return true;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('n') {
            self.prompt_new_project();
            return true;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('o') {
            self.toggle_all_activity();
            return true;
        }
        // Cmd+V (Super), for terminals that report it.
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
            && key.code == KeyCode::Char('v')
        {
            self.request_paste();
            return true;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('g') {
            self.edit_input_in_editor();
            return true;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('t') {
            self.stop_command();
            return true;
        }
        false
    }

    /// Alt+Up/Down switch agents, Alt+T cycles thinking and Alt+digit
    /// opens an agents tab. Returns whether the key was used.
    fn alt_shortcut_key(&mut self, key: KeyEvent) -> bool {
        if !key.modifiers.contains(KeyModifiers::ALT) {
            return false;
        }
        match key.code {
            KeyCode::Up => self.switch_agent_by(-1),
            KeyCode::Down => self.switch_agent_by(1),
            KeyCode::Char('t') => self.cycle_thinking(),
            KeyCode::Char(digit) => {
                let Some(tab) = AgentsTab::numbered(digit) else {
                    return false;
                };
                self.show_agents_tab(tab);
            }
            _ => return false,
        }
        true
    }

    /// The focused panel's own shortcuts. Returns whether the key was used.
    fn panel_shortcut_key(&mut self, key: KeyEvent, word_motion: bool) -> bool {
        self.swarm_shortcut_key(key, word_motion)
            || self.chat_panel_shortcut_key(key)
            || self.list_panel_shortcut_key(key, word_motion)
    }

    fn swarm_shortcut_key(&mut self, key: KeyEvent, word_motion: bool) -> bool {
        self.focus == PanelFocus::Swarm
            && self.input.text.is_empty()
            && self.agents_panel_shortcut(key, word_motion)
    }

    /// Shortcuts of the chat panel's tabs (and the benchmark form, which
    /// fills from any panel on Enter).
    fn chat_panel_shortcut_key(&mut self, key: KeyEvent) -> bool {
        if self.focus == PanelFocus::Chat
            && self.input.text.is_empty()
            && key.modifiers.contains(KeyModifiers::SHIFT)
            && matches!(key.code, KeyCode::Left | KeyCode::Right)
        {
            self.shift_chat_tab(key.code == KeyCode::Right);
            return true;
        }
        if self.focus == PanelFocus::Chat
            && matches!(self.chat_tab(), ChatTab::Graph | ChatTab::Time)
            && self.input.text.is_empty()
            && self.handle_flow_key(key)
        {
            return true;
        }
        if self.bench_app() {
            // Typed with the list's keys too, text still fills the form.
            if key.code == KeyCode::Enter && key.modifiers.is_empty() && self.bench_fill_field() {
                return true;
            }
            if self.focus == PanelFocus::Chat
                && self.input.text.is_empty()
                && self.handle_bench_key(key)
            {
                return true;
            }
        }
        self.focus == PanelFocus::Chat
            && self.chat_tab() == ChatTab::Log
            && self.detail.view.is_none()
            && self.input.text.is_empty()
            && self.handle_log_key(key)
    }

    /// Shortcuts of the side panel and the records panel.
    fn list_panel_shortcut_key(&mut self, key: KeyEvent, word_motion: bool) -> bool {
        if self.focus == PanelFocus::Sandbox
            && self.input.text.is_empty()
            && !word_motion
            && (self.handle_side_tab_key(key) || self.handle_side_list_shortcut(key))
        {
            return true;
        }
        self.focus == PanelFocus::Records
            && self.input.text.is_empty()
            && !word_motion
            && self.handle_records_shortcut(key)
    }

    /// Typing, and editing or sending a non-empty prompt, goes to the chat
    /// input from any panel.
    fn typing_key(&mut self, key: KeyEvent, word_motion: bool) -> Option<KeyResult> {
        match key.code {
            KeyCode::Char(_)
            | KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Home
            | KeyCode::End => Some(self.handle_chat_key(key)),
            KeyCode::Left | KeyCode::Right if word_motion || !self.input.text.is_empty() => {
                Some(self.handle_chat_key(key))
            }
            KeyCode::Enter if !self.input.text.is_empty() => Some(self.handle_chat_key(key)),
            _ => None,
        }
    }

    /// Shift+Enter, or Alt+Enter or Ctrl+J, breaks the line at the cursor
    /// in the chat or terminal prompt. Typing goes to the input from any
    /// panel, so the break does too, empty line or not. The one exception
    /// is an empty line while the focused chat panel shows GRAPH, TIME or
    /// LOG: those tabs keep the key for their rows (Shift+Enter opens
    /// agent cards on GRAPH). Returns whether the key was used.
    fn insert_newline_key(&mut self, key: KeyEvent) -> bool {
        let tab_owns_keys = self.focus == PanelFocus::Chat
            && !matches!(self.chat_tab(), ChatTab::Chat | ChatTab::Terminal);
        let prompt_owns_keys =
            self.queue_selected().is_none() && (!self.input.text.is_empty() || !tab_owns_keys);
        if !(is_newline_key(key) && prompt_owns_keys) {
            return false;
        }
        self.insert_newline();
        self.input.reset_slash_menu();
        true
    }

    /// `$` on an empty line opens the TERMINAL tab; Esc, or Backspace on an
    /// empty line, goes back to CHAT. Returns whether the key was used.
    fn toggle_terminal_mode(&mut self, key: KeyEvent) -> bool {
        let entering = !self.terminal_mode
            && key.code == KeyCode::Char('$')
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && self.input.text.is_empty();
        let leaving = self.terminal_mode
            && (key.code == KeyCode::Esc
                || (key.code == KeyCode::Backspace && self.input.text.is_empty()));
        if !(entering || leaving) {
            return false;
        }
        self.show_chat_tab(if entering {
            ChatTab::Terminal
        } else {
            ChatTab::Chat
        });
        true
    }
}
