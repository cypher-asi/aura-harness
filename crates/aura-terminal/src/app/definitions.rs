//! The agents panel's tabs, and its roster of permanent agents: selecting,
//! spawning, editing, and deleting them.

use super::{contains, follow_selection, App, AppState, NotificationType};
use crate::events::{AgentDefinitionSummary, UiEvent};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use std::path::PathBuf;

/// Views of the agents panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentsTab {
    /// Live agents: the ones the user talks to and the instances they spawned.
    #[default]
    Projects,
    /// The roster of permanent agents that can be spawned by name.
    Roster,
}

impl AgentsTab {
    pub const ALL: [Self; 2] = [Self::Projects, Self::Roster];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Projects => "Projects",
            Self::Roster => "Agents",
        }
    }

    const fn other(self) -> Self {
        match self {
            Self::Projects => Self::Roster,
            Self::Roster => Self::Projects,
        }
    }

    /// Each tab with the first column and width of its ` label ` cell in a
    /// tab bar that starts with one column of margin. The renderer draws
    /// the bar with this layout and clicks are mapped back through it.
    pub fn layout() -> impl Iterator<Item = (Self, u16, u16)> {
        let mut x = 1u16;
        Self::ALL.into_iter().map(move |tab| {
            let width = u16::try_from(tab.label().len() + 2).unwrap_or(u16::MAX);
            let start = x;
            x = x.saturating_add(width);
            (tab, start, width)
        })
    }
}

impl App {
    /// Which list the agents panel shows.
    #[must_use]
    pub const fn agents_tab(&self) -> AgentsTab {
        self.agents_tab
    }

    /// Record where the agents panel's tab bar was drawn this frame.
    pub fn set_agent_tabs_area(&mut self, area: Rect) {
        self.areas.agent_tabs = Some(area);
    }

    pub(super) fn show_agents_tab(&mut self, tab: AgentsTab) {
        self.agents_tab = tab;
        self.pending_delete = None;
    }

    pub(super) fn toggle_agents_tab(&mut self) {
        self.show_agents_tab(self.agents_tab.other());
    }

    /// Clicking a tab shows it; clicking a row acts on the visible list.
    pub(super) fn click_agents_panel(&mut self, column: u16, row: u16) {
        if let Some(bar) = self
            .areas
            .agent_tabs
            .filter(|&bar| contains(Some(bar), column, row))
        {
            let offset = column - bar.x;
            if let Some((tab, _, _)) = AgentsTab::layout()
                .find(|&(_, start, width)| offset >= start && offset < start.saturating_add(width))
            {
                self.show_agents_tab(tab);
            }
            return;
        }
        match self.agents_tab {
            AgentsTab::Projects => self.click_agent_row(column, row),
            AgentsTab::Roster => self.click_definition_row(column, row),
        }
    }
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
        // The new instance becomes the active agent, listed under Projects.
        self.show_agents_tab(AgentsTab::Projects);
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
            KeyCode::Left | KeyCode::Right => self.toggle_agents_tab(),
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
