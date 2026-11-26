//! The AGENTS app: the roster of permanent agents, and selecting,
//! spawning, editing, and deleting them.

use crate::app::{contains, follow_selection, AgentsTab, App, NotificationType};
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

    /// Highlighted row in the roster (the last row is
    /// "+ New agent").
    #[must_use]
    pub const fn selected_definition(&self) -> usize {
        self.selected_definition
    }

    /// First visible row of the roster.
    #[must_use]
    pub const fn definitions_scroll(&self) -> usize {
        self.definitions_scroll
    }

    /// Keep the highlighted row inside a list `visible_rows` tall.
    /// "+ New agent" is pinned below the list, so it never scrolls.
    pub fn sync_definitions_scroll(&mut self, visible_rows: usize) {
        self.definitions_scroll = follow_selection(
            self.definitions_scroll,
            self.selected_definition,
            visible_rows,
            self.definitions.len(),
        );
    }

    /// Record where the permanent agents rows were drawn this frame.
    pub fn set_definition_rows_area(&mut self, area: Rect) {
        self.areas.definition_rows = Some(area);
    }

    /// Record where "+ New agent" was drawn this frame.
    pub fn set_new_agent_area(&mut self, area: Rect) {
        self.areas.new_agent_row = Some(area);
    }

    /// Roster row drawn at a screen cell: a permanent agent, or the pinned
    /// "+ New agent" (index `definitions.len()`).
    pub(in crate::app) fn definition_row_at(&self, column: u16, row: u16) -> Option<usize> {
        if contains(self.areas.new_agent_row, column, row) {
            return Some(self.definitions.len());
        }
        let rows = self
            .areas
            .definition_rows
            .filter(|&rows| contains(Some(rows), column, row))?;
        let index = self.definitions_scroll + usize::from(row - rows.y);
        (index < self.definitions.len()).then_some(index)
    }

    /// File the user asked to edit, once. The terminal opens it in the
    /// editor and then calls [`Self::editor_closed`].
    pub fn take_editor_request(&mut self) -> Option<PathBuf> {
        self.editor_request.take()
    }

    /// Report that the editor opened for `path` exited.
    pub fn editor_closed(&mut self, path: PathBuf, result: &std::io::Result<()>) {
        self.redraw = true;
        if self.input_editor_closed(&path, result) {
            return;
        }
        if let Err(e) = result {
            self.notification = Some((
                format!("Could not open editor: {e}"),
                NotificationType::Error,
            ));
        }
        self.send(UiEvent::DefinitionEdited(path));
    }

    pub(in crate::app) fn set_definitions(&mut self, definitions: Vec<AgentDefinitionSummary>) {
        self.definitions = definitions;
        self.selected_definition = self.selected_definition.min(self.definitions.len());
    }

    pub(in crate::app) fn select_definition_row(&mut self, row: usize) {
        self.selected_definition = row.min(self.definitions.len());
    }

    pub(in crate::app) fn selected_definition_name(&self) -> Option<String> {
        self.definitions
            .get(self.selected_definition)
            .map(|d| d.name.clone())
    }

    /// Spawn the highlighted permanent agent, or create one on the
    /// "+ New agent" row.
    pub(in crate::app) fn activate_definition_row(&mut self) {
        match self.selected_definition_name() {
            Some(name) => self.instantiate_definition(&name),
            None => self.edit_definition(None),
        }
    }

    /// Start an instance of the permanent agent `name` and switch to it.
    pub(in crate::app) fn instantiate_definition(&mut self, name: &str) {
        self.send(UiEvent::InstantiateDefinition(name.to_string()));
        // The new instance becomes the active agent, listed under Projects.
        self.switch_app(AgentsTab::Projects);
    }

    /// Add an instance of the permanent agent `name` to the active agent's
    /// project and switch to it. Agents run in parallel, so this works
    /// mid-turn too.
    pub(in crate::app) fn add_definition_to_project(&mut self, name: &str) {
        self.send(UiEvent::AddDefinitionToProject(name.to_string()));
        self.switch_app(AgentsTab::Projects);
    }

    /// Open the permanent agent `name` in the editor, or a new one for `None`.
    pub(in crate::app) fn edit_definition(&mut self, name: Option<String>) {
        self.send(UiEvent::EditDefinition(name));
    }

    /// Ask to confirm deleting the highlighted permanent agent.
    fn request_delete_selected(&mut self) {
        let Some(name) = self.selected_definition_name() else {
            return;
        };
        self.notification = Some((
            format!("Delete permanent agent {name}? Press y to confirm"),
            NotificationType::Warning,
        ));
        self.pending_delete = Some(name);
    }

    /// Resolve a pending deletion with `key`. Returns `false` when nothing
    /// was pending.
    pub(in crate::app) fn handle_pending_delete(&mut self, key: KeyEvent) -> bool {
        let Some(name) = self.pending_delete.take() else {
            return false;
        };
        if matches!(key.code, KeyCode::Char('y' | 'Y')) {
            self.send(UiEvent::DeleteDefinition(name));
        }
        true
    }

    /// Letter shortcuts of the focused panel, used while the input is empty.
    pub(in crate::app) fn handle_definitions_shortcut(&mut self, key: KeyEvent) -> bool {
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
    pub(in crate::app) fn handle_definitions_key(&mut self, key: KeyEvent) {
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
            KeyCode::Left | KeyCode::Right => self.shift_agents_tab(key.code),
            _ => {}
        }
    }

    /// Clicking a row highlights it and shows its latest instance's chat;
    /// clicking "+ New agent" creates one.
    pub(in crate::app) fn click_definition_row(&mut self, column: u16, row: u16) {
        let Some(index) = self.definition_row_at(column, row) else {
            return;
        };
        self.selected_definition = index;
        match self.selected_definition_name() {
            Some(name) => self.open_definition(&name),
            None => self.edit_definition(None),
        }
    }

    fn send(&self, event: UiEvent) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(event);
        }
    }
}
