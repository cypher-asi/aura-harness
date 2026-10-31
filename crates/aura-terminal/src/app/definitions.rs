//! Permanent agents panel: selecting, spawning, editing, and deleting
//! permanent agents.

use super::{contains, follow_selection, App, AppState, NotificationType};
use crate::events::{AgentDefinitionSummary, UiEvent};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use std::path::PathBuf;

impl App {
    /// Permanent agents shown in the panel.
    #[must_use]
    pub fn definitions(&self) -> &[AgentDefinitionSummary] {
        &self.definitions
    }

    /// Highlighted row in the Permanent agents panel (the last row is
    /// "+ New permanent").
    #[must_use]
    pub const fn selected_definition(&self) -> usize {
        self.selected_definition
    }

    /// First visible row of the Permanent agents panel.
    #[must_use]
    pub const fn definitions_scroll(&self) -> usize {
        self.definitions_scroll
    }

    /// Keep the highlighted row inside a list `visible_rows` tall.
    pub fn sync_definitions_scroll(&mut self, visible_rows: usize) {
        self.definitions_scroll = follow_selection(
            self.definitions_scroll,
            self.selected_definition,
            visible_rows,
            self.definitions.len() + 1,
        );
    }

    /// Record where the permanent agents rows were drawn this frame.
    pub fn set_definition_rows_area(&mut self, area: Rect) {
        self.areas.definition_rows = Some(area);
    }

    /// File the user asked to edit, once. The terminal opens it in the
    /// editor and then calls [`Self::editor_closed`].
    pub fn take_editor_request(&mut self) -> Option<PathBuf> {
        self.editor_request.take()
    }

    /// Report that the editor opened for `path` exited.
    pub fn editor_closed(&mut self, path: PathBuf, result: &std::io::Result<()>) {
        if let Err(e) = result {
            self.notification = Some((
                format!("Could not open editor: {e}"),
                NotificationType::Error,
            ));
        }
        self.send(UiEvent::DefinitionEdited(path));
    }

    pub(super) fn set_definitions(&mut self, definitions: Vec<AgentDefinitionSummary>) {
        self.definitions = definitions;
        self.selected_definition = self.selected_definition.min(self.definitions.len());
    }

    pub(super) fn select_definition_row(&mut self, row: usize) {
        self.selected_definition = row.min(self.definitions.len());
    }

    fn selected_definition_name(&self) -> Option<String> {
        self.definitions
            .get(self.selected_definition)
            .map(|d| d.name.clone())
    }

    /// Spawn the highlighted permanent agent, or create one on the
    /// "+ New permanent" row.
    pub(super) fn activate_definition_row(&mut self) {
        match self.selected_definition_name() {
            Some(name) => self.instantiate_definition(&name),
            None => self.edit_definition(None),
        }
    }

    /// Start an instance of the permanent agent `name` and switch to it.
    pub(super) fn instantiate_definition(&mut self, name: &str) {
        if self.refuse_while_busy("starting a permanent agent") {
            return;
        }
        self.send(UiEvent::InstantiateDefinition(name.to_string()));
    }

    /// Open the permanent agent `name` in the editor, or a new one for `None`.
    pub(super) fn edit_definition(&mut self, name: Option<String>) {
        if self.refuse_while_busy("editing permanent agents") {
            return;
        }
        self.send(UiEvent::EditDefinition(name));
    }

    /// Ask to confirm deleting the highlighted permanent agent.
    fn request_delete_selected(&mut self) {
        let Some(name) = self.selected_definition_name() else {
            return;
        };
        if self.refuse_while_busy("deleting permanent agents") {
            return;
        }
        self.notification = Some((
            format!("Delete permanent agent {name}? Press y to confirm"),
            NotificationType::Warning,
        ));
        self.pending_delete = Some(name);
    }

    /// Resolve a pending deletion with `key`. Returns `false` when nothing
    /// was pending.
    pub(super) fn handle_pending_delete(&mut self, key: KeyEvent) -> bool {
        let Some(name) = self.pending_delete.take() else {
            return false;
        };
        if matches!(key.code, KeyCode::Char('y' | 'Y')) {
            self.send(UiEvent::DeleteDefinition(name));
        }
        true
    }

    /// Letter shortcuts of the focused panel, used while the input is empty.
    pub(super) fn handle_definitions_shortcut(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('e') => {
                let name = self.selected_definition_name();
                self.edit_definition(name);
            }
            KeyCode::Char('n') => self.edit_definition(None),
            KeyCode::Char('d') | KeyCode::Delete => self.request_delete_selected(),
            _ => return false,
        }
        true
    }

    /// Navigation keys of the focused panel.
    pub(super) fn handle_definitions_key(&mut self, key: KeyEvent) {
        let last = self.definitions.len();
        match key.code {
            KeyCode::Up => self.select_definition_row(self.selected_definition.saturating_sub(1)),
            KeyCode::Down => self.select_definition_row(self.selected_definition + 1),
            KeyCode::PageUp => {
                self.select_definition_row(self.selected_definition.saturating_sub(5));
            }
            KeyCode::PageDown => self.select_definition_row(self.selected_definition + 5),
            KeyCode::Home => self.select_definition_row(0),
            KeyCode::End => self.select_definition_row(last),
            KeyCode::Enter => self.activate_definition_row(),
            _ => {}
        }
    }

    /// Clicking a row highlights it; clicking "+ New permanent"
    /// creates one.
    pub(super) fn click_definition_row(&mut self, column: u16, row: u16) {
        let Some(rows) = self.areas.definition_rows else {
            return;
        };
        if !contains(Some(rows), column, row) {
            return;
        }
        let index = self.definitions_scroll + usize::from(row - rows.y);
        if index > self.definitions.len() {
            return;
        }
        self.selected_definition = index;
        if index == self.definitions.len() {
            self.edit_definition(None);
        }
    }

    /// Warn and return `true` while a turn is in flight: the backend
    /// handles one request at a time.
    fn refuse_while_busy(&mut self, action: &str) -> bool {
        let busy = matches!(
            self.state,
            AppState::Processing | AppState::AwaitingApproval
        );
        if busy {
            self.notification = Some((
                format!("Wait for the current turn to finish before {action}"),
                NotificationType::Warning,
            ));
        }
        busy
    }

    fn send(&self, event: UiEvent) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(event);
        }
    }
}
