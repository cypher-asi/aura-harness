//! The AGENTS app: the roster of permanent agents, and selecting,
//! spawning, editing, and deleting them.
//!
//! An instance of a permanent agent is any agent made from it: a listed
//! one, an archived one, or a member of a channel, which the projects
//! list leaves out unless its channel works in a project. Each
//! channel's coordinator is an instance of `coordinator`, which only
//! making a channel starts: Enter on it opens its latest instance, as a
//! click does, instead of asking the session for a new one.

use crate::app::wheel_views::WheelList;
use crate::app::{contains, AgentsTab, App, NotificationType};
use crate::events::{AgentDefinitionSummary, AgentSummary, UiEvent};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use std::path::PathBuf;

/// The permanent agent every channel's coordinator is an instance of.
const COORDINATOR_DEFINITION: &str = "coordinator";

/// What AGENTS says about `coordinator` with no instance: only making a
/// channel starts one.
const NO_COORDINATOR: &str = "No coordinator has run yet. Make a channel to start one.";

/// What the chat says in AGENTS when the permanent agent `name` has no
/// instance.
pub(super) fn no_instance_message(name: &str) -> String {
    if name == COORDINATOR_DEFINITION {
        NO_COORDINATOR.to_string()
    } else {
        format!("No {name} agent has run yet. Enter on it starts one.")
    }
}

impl App {
    /// Permanent agents shown in the panel.
    #[must_use]
    pub fn definitions(&self) -> &[AgentDefinitionSummary] {
        &self.definitions.items
    }

    /// Highlighted row in the roster (the last row is
    /// "+ New agent").
    #[must_use]
    pub const fn selected_definition(&self) -> usize {
        self.definitions.selected
    }

    /// First visible row of the roster.
    #[must_use]
    pub const fn definitions_scroll(&self) -> usize {
        self.definitions.scroll
    }

    /// Keep the highlighted row inside a list `visible_rows` tall.
    /// "+ New agent" is pinned below the list, so it never scrolls.
    pub fn sync_definitions_scroll(&mut self, visible_rows: usize) {
        let list = &self.definitions;
        self.definitions.scroll = self.panels.wheel.place(
            WheelList::Roster,
            (list.scroll, list.selected),
            (visible_rows, list.len()),
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
        let index = self.definitions.scroll + usize::from(row - rows.y);
        (index < self.definitions.len()).then_some(index)
    }

    /// File the user asked to edit, once. The terminal opens it in the
    /// editor and then calls [`Self::editor_closed`].
    pub fn take_editor_request(&mut self) -> Option<PathBuf> {
        self.pending.editor_request.take()
    }

    /// Report that the editor opened for `path` exited.
    pub fn editor_closed(&mut self, path: PathBuf, result: &std::io::Result<()>) {
        self.ticks.redraw = true;
        if self.input_editor_closed(&path, result) {
            return;
        }
        if let Err(e) = result {
            self.notifications.current = Some((
                format!("Could not open editor: {e}"),
                NotificationType::Error,
            ));
        }
        self.send(UiEvent::DefinitionEdited(path));
    }

    pub(in crate::app) fn set_definitions(&mut self, definitions: Vec<AgentDefinitionSummary>) {
        self.definitions.items = definitions;
        self.definitions.selected = self.definitions.selected.min(self.definitions.len());
    }

    pub(in crate::app) fn select_definition_row(&mut self, row: usize) {
        self.definitions.selected = row.min(self.definitions.len());
    }

    pub(in crate::app) fn selected_definition_name(&self) -> Option<String> {
        self.definitions.selected_item().map(|d| d.name.clone())
    }

    /// Spawn the highlighted permanent agent, or create one on the
    /// "+ New agent" row. `coordinator` is not spawned: its latest
    /// instance opens, as a click does.
    pub(in crate::app) fn activate_definition_row(&mut self) {
        match self.selected_definition_name() {
            Some(name) if name == COORDINATOR_DEFINITION => self.open_coordinator(),
            Some(name) => self.instantiate_definition(&name),
            None => self.edit_definition(None),
        }
    }

    /// Open the latest channel coordinator in AGENTS, or say that making
    /// a channel starts one. The session refuses to start one by itself.
    fn open_coordinator(&mut self) {
        self.open_definition(COORDINATOR_DEFINITION);
        if self.latest_instance(COORDINATOR_DEFINITION).is_none() {
            self.notifications.current =
                Some((NO_COORDINATOR.to_string(), NotificationType::Warning));
        }
    }

    /// Every agent that can be an instance of a permanent agent, each
    /// once, with whether it is listed: the listed ones, the archived
    /// ones, then the channel members neither of those holds.
    fn known_agents(&self) -> impl Iterator<Item = (&AgentSummary, bool)> + '_ {
        let listed = self.agents.iter().map(|agent| (agent, true));
        let archived = self.archived.items.iter().map(|agent| (agent, false));
        let members = self
            .channels()
            .iter()
            .flat_map(|channel| channel.members.iter())
            .filter(|member| !self.is_listed(&member.id) && !self.is_archived_agent(&member.id))
            .map(|member| (member, false));
        listed.chain(archived).chain(members)
    }

    /// Latest instance of the permanent agent `name`, listed, archived or
    /// in a channel: the one that last did something, a listed one on a
    /// tie.
    #[must_use]
    pub(crate) fn latest_instance(&self, name: &str) -> Option<&str> {
        self.known_agents()
            .filter(|(agent, _)| agent.definition.as_deref() == Some(name))
            .max_by_key(|(agent, listed)| (agent.last_active_ms, *listed))
            .map(|(agent, _)| agent.id.as_str())
    }

    /// Show the latest instance of the permanent agent `name` in AGENTS,
    /// or that it has none. A channel's member comes on screen as CHANNELS
    /// puts its coordinator there, though the projects list leaves it out.
    pub(in crate::app) fn open_definition(&mut self, name: &str) {
        self.apps.roster = Some(name.to_string());
        if let Some(id) = self.latest_instance(name).map(str::to_string) {
            self.switch_on_screen(&id);
        }
    }

    /// Whether the agent on screen is an instance of the permanent agent
    /// AGENTS shows.
    pub(super) fn roster_shows_instance(&self) -> bool {
        let Some(name) = self.apps.roster.as_deref() else {
            return false;
        };
        self.known_agents().any(|(agent, _)| {
            agent.id == self.active_agent_id && agent.definition.as_deref() == Some(name)
        })
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
        self.notifications.current = Some((
            format!("Delete permanent agent {name}? Press y to confirm"),
            NotificationType::Warning,
        ));
        self.pending.delete = Some(name);
    }

    /// Resolve a pending deletion with `key`. Returns `false` when nothing
    /// was pending.
    pub(in crate::app) fn handle_pending_delete(&mut self, key: KeyEvent) -> bool {
        let Some(name) = self.pending.delete.take() else {
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
        let selected = self.definitions.selected;
        match key.code {
            KeyCode::Up => self.select_definition_row(selected.saturating_sub(1)),
            KeyCode::Down => self.select_definition_row(selected + 1),
            KeyCode::PageUp => {
                self.select_definition_row(selected.saturating_sub(5));
            }
            KeyCode::PageDown => self.select_definition_row(selected + 5),
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
        self.definitions.selected = index;
        match self.selected_definition_name() {
            Some(name) => self.open_definition(&name),
            None => self.edit_definition(None),
        }
    }

    fn send(&self, event: UiEvent) {
        if let Some(tx) = &self.link.event_tx {
            let _ = tx.try_send(event);
        }
    }
}
