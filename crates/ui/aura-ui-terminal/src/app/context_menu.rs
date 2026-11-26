//! The menu a right click opens on a project or an agent in the projects
//! list, or on an agent in the Archive. Its items are the list's own
//! actions (add an agent, project info, fold, archive, remove, unarchive),
//! gathered where the pointer is.
//!
//! The menu keeps the ids of what it was opened on, not list positions, so
//! the list may change under it. A click on an item or Enter runs it; Esc,
//! any other key or a click elsewhere closes it.
//!
//! When the host has workspaces (the desktop's tabs, see
//! [`App::set_workspaces`]), projects and agents also get "Move to
//! workspace", whose submenu lists the other workspaces and a new one.
//! A new one is left out for the only project listed, which has a
//! workspace of its own already, and the item itself when that leaves
//! nothing to pick.
//! Picking one queues a [`WorkspaceMove`] for the host to take with
//! [`App::take_workspace_move`]; the session hears nothing of it, since
//! workspaces are the host's alone. An agent moves with its whole project,
//! because a workspace holds projects, not single agents.
//!
//! On the CHANNELS tab the menu opens on a channel row of the list, or
//! anywhere in the open channel's INFO side tab, and offers "Edit
//! channel…", which opens the channel form on it (title, description and
//! project), and "Archive channel…", which asks for `y` first.

use super::{contains, AgentsTab, App, AppState, PanelFocus, ProjectListRow};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Position, Rect};

/// Where a project is to go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceTarget {
    /// The workspace at this index of the labels given to
    /// [`App::set_workspaces`]
    Existing(usize),
    /// A workspace of its own, to be opened for it
    New,
}

/// A project the user asked to move to another workspace, from the
/// projects list's right-click menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceMove {
    /// Key of the project (see [`crate::events::AgentSummary::project_key`])
    pub project: String,
    /// Where it goes
    pub to: WorkspaceTarget,
}

/// The workspaces the host has, as it last told.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Workspaces {
    /// Each workspace's title, in order; may be empty
    labels: Vec<String>,
    /// Index of the workspace this UI is
    current: usize,
}

impl Workspaces {
    /// What the submenu offers: every other workspace, then a new one
    /// when `offer_new`. Nothing without workspaces, so a plain terminal
    /// has no such menu.
    fn targets(&self, offer_new: bool) -> Vec<WorkspaceTarget> {
        if self.labels.is_empty() {
            return Vec::new();
        }
        (0..self.labels.len())
            .filter(|&index| index != self.current)
            .map(WorkspaceTarget::Existing)
            .chain(offer_new.then_some(WorkspaceTarget::New))
            .collect()
    }
}

/// An entry of a submenu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SubmenuEntry {
    /// Where "Move to workspace" sends the project
    Workspace(WorkspaceTarget),
}

/// The list an item with a submenu opens beside the menu: workspaces for
/// "Move to workspace".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Submenu {
    pub entries: Vec<SubmenuEntry>,
    /// Highlighted entry
    pub selected: usize,
    /// Where the submenu was drawn in the last frame
    pub area: Option<Rect>,
}

impl Submenu {
    fn entry_at(&self, column: u16, row: u16) -> Option<usize> {
        row_at(self.area, self.entries.len(), column, row)
    }

    fn covers(&self, column: u16, row: u16) -> bool {
        contains(self.area, column, row)
    }
}

/// The row of a bordered menu drawn in `area` at a screen cell, inside the
/// border, when one of its `len` rows is there.
fn row_at(area: Option<Rect>, len: usize, column: u16, row: u16) -> Option<usize> {
    let area = area?;
    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    if !contains(Some(inner), column, row) {
        return None;
    }
    let index = usize::from(row - inner.y);
    (index < len).then_some(index)
}

/// The highlight after a navigation key in a list of `len` rows, wrapping
/// at the ends; `None` for any other key.
fn step(selected: usize, len: usize, code: KeyCode) -> Option<usize> {
    let last = len.saturating_sub(1);
    match code {
        KeyCode::Up => Some(selected.checked_sub(1).unwrap_or(last)),
        KeyCode::Down => Some(if selected >= last { 0 } else { selected + 1 }),
        KeyCode::Home => Some(0),
        KeyCode::End => Some(last),
        _ => None,
    }
}

/// What the menu was opened on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MenuTarget {
    /// A project, by its key (see [`crate::events::AgentSummary::project_key`])
    Project(String),
    /// An agent, by id
    Agent(String),
    /// An agent in the Archive, by id. Kept apart from [`Self::Agent`]
    /// because archived agents are not in the projects list, so its
    /// items must not look the id up there.
    Archived(String),
    /// A channel, by id
    Channel(String),
}

/// An entry of the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MenuItem {
    AddAgent,
    Info,
    Fold,
    Open,
    Archive,
    /// Opens the submenu of workspaces
    MoveToWorkspace,
    RemoveProject,
    Restore,
    /// Opens the channel form on the channel
    EditChannel,
    ArchiveChannel,
}

impl MenuItem {
    /// Whether the item opens a submenu rather than running.
    pub(crate) fn has_submenu(self) -> bool {
        matches!(self, Self::MoveToWorkspace)
    }
}

/// The open menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextMenu {
    pub target: MenuTarget,
    pub items: Vec<MenuItem>,
    /// The cell that was clicked; the menu opens below and right of it
    pub anchor: Position,
    /// Highlighted item
    pub selected: usize,
    /// Where the menu was drawn in the last frame
    pub area: Option<Rect>,
    /// The workspaces "Move to workspace" opened, while open
    pub submenu: Option<Submenu>,
}

impl ContextMenu {
    fn new(target: MenuTarget, items: Vec<MenuItem>, anchor: Position) -> Self {
        Self {
            target,
            items,
            anchor,
            selected: 0,
            area: None,
            submenu: None,
        }
    }

    /// The item drawn at a screen cell, inside the menu's border.
    fn item_at(&self, column: u16, row: u16) -> Option<usize> {
        row_at(self.area, self.items.len(), column, row)
    }

    fn covers(&self, column: u16, row: u16) -> bool {
        contains(self.area, column, row)
    }

    /// Whether the menu or its submenu is drawn over a screen cell.
    fn covers_any(&self, column: u16, row: u16) -> bool {
        self.covers(column, row)
            || self
                .submenu
                .as_ref()
                .is_some_and(|submenu| submenu.covers(column, row))
    }

    fn highlights_submenu_item(&self) -> bool {
        self.items
            .get(self.selected)
            .is_some_and(|item| item.has_submenu())
    }
}

impl App {
    /// The open context menu, if any.
    pub(crate) const fn context_menu(&self) -> Option<&ContextMenu> {
        self.context_menu.as_ref()
    }

    /// Record where the context menu was drawn this frame.
    pub(crate) fn set_context_menu_area(&mut self, area: Rect) {
        if let Some(menu) = self.context_menu.as_mut() {
            menu.area = Some(area);
        }
    }

    /// Record where the context menu's submenu was drawn this frame.
    pub(crate) fn set_context_submenu_area(&mut self, area: Rect) {
        if let Some(submenu) = self.context_menu.as_mut().and_then(|m| m.submenu.as_mut()) {
            submenu.area = Some(area);
        }
    }

    /// Tell the UI which workspaces its host has, in order, each by its
    /// title (which may be empty), and which of them this UI is. With
    /// some, the right-click menu of a project or agent offers to move
    /// it to another one; see [`Self::take_workspace_move`]. Without any,
    /// the default, it doesn't.
    pub fn set_workspaces(&mut self, labels: Vec<String>, current: usize) {
        let workspaces = Workspaces { labels, current };
        if workspaces == self.workspaces {
            return;
        }
        self.workspaces = workspaces;
        self.redraw = true;
        let targets = match self.context_menu.as_ref() {
            Some(menu) if menu.items.contains(&MenuItem::MoveToWorkspace) => {
                self.workspace_targets(&menu.target)
            }
            _ => return,
        };
        let Some(menu) = self.context_menu.as_mut() else {
            return;
        };
        if targets.is_empty() {
            // The item would lead nowhere now.
            menu.submenu = None;
            menu.items.retain(|&item| item != MenuItem::MoveToWorkspace);
            menu.selected = menu.selected.min(menu.items.len().saturating_sub(1));
        } else if let Some(submenu) = menu.submenu.as_mut() {
            submenu.selected = submenu.selected.min(targets.len() - 1);
            submenu.entries = targets.into_iter().map(SubmenuEntry::Workspace).collect();
        }
    }

    /// The move to another workspace the user picked since the last call,
    /// if any. The host does it: the UI only asks.
    pub fn take_workspace_move(&mut self) -> Option<WorkspaceMove> {
        self.workspace_move.take()
    }

    /// The workspaces' titles, as the host last gave them.
    pub(crate) fn workspace_labels(&self) -> &[String] {
        &self.workspaces.labels
    }

    /// The workspaces as the host last told them with
    /// [`Self::set_workspaces`]: their titles, and the index of this UI's.
    #[must_use]
    pub fn workspaces(&self) -> (&[String], usize) {
        (&self.workspaces.labels, self.workspaces.current)
    }

    /// Where "Move to workspace" can send what the menu was opened on.
    /// A new workspace is left out for the only project listed: it has
    /// a workspace of its own already. Nothing for an archived agent.
    fn workspace_targets(&self, target: &MenuTarget) -> Vec<WorkspaceTarget> {
        match target {
            MenuTarget::Project(_) | MenuTarget::Agent(_) => {
                self.workspaces.targets(self.listed_project_count() > 1)
            }
            MenuTarget::Archived(_) | MenuTarget::Channel(_) => Vec::new(),
        }
    }

    /// What the highlighted item's submenu lists.
    fn submenu_entries(&self, menu: &ContextMenu) -> Vec<SubmenuEntry> {
        match menu.items.get(menu.selected) {
            Some(MenuItem::MoveToWorkspace) => self
                .workspace_targets(&menu.target)
                .into_iter()
                .map(SubmenuEntry::Workspace)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// How many projects the projects list has a header for, those with
    /// every agent archived included.
    fn listed_project_count(&self) -> usize {
        self.project_rows()
            .iter()
            .filter(|row| {
                matches!(
                    row,
                    ProjectListRow::Project { .. } | ProjectListRow::EmptyProject { .. }
                )
            })
            .count()
    }

    /// What an item reads, for the target it was opened on.
    pub(crate) fn menu_label(&self, menu: &ContextMenu, item: MenuItem) -> &'static str {
        match item {
            MenuItem::AddAgent => "Add agent",
            MenuItem::Info => "Project info",
            MenuItem::Fold => {
                let folded = match &menu.target {
                    MenuTarget::Project(id) => self.collapsed_projects.contains(id),
                    MenuTarget::Agent(id) => self.collapsed_agents.contains(id),
                    MenuTarget::Archived(_) | MenuTarget::Channel(_) => false,
                };
                if folded {
                    "Expand"
                } else {
                    "Collapse"
                }
            }
            MenuItem::Open => "Open",
            MenuItem::Archive => "Archive",
            MenuItem::MoveToWorkspace => "Move to workspace",
            MenuItem::RemoveProject => "Remove project…",
            MenuItem::Restore => "Unarchive",
            MenuItem::EditChannel => "Edit channel…",
            MenuItem::ArchiveChannel => "Archive channel…",
        }
    }

    /// `items` with "Move to workspace" before "Remove project…", when
    /// the host has a workspace `target` can move to.
    fn with_workspace_item(&self, target: &MenuTarget, mut items: Vec<MenuItem>) -> Vec<MenuItem> {
        if !self.workspace_targets(target).is_empty() {
            let at = items
                .iter()
                .position(|&item| item == MenuItem::RemoveProject)
                .unwrap_or(items.len());
            items.insert(at, MenuItem::MoveToWorkspace);
        }
        items
    }

    /// A right click on the agents panel: highlight the project or agent
    /// under the pointer and open its menu there. Returns whether a menu
    /// opened.
    pub(super) fn open_context_menu(&mut self, column: u16, row: u16) -> bool {
        self.context_menu = None;
        match self.agents_tab {
            AgentsTab::Projects => {}
            AgentsTab::Archive => return self.open_archive_menu(column, row),
            AgentsTab::Channels => return self.open_channel_menu(column, row),
            AgentsTab::Roster => return false,
        }
        let Some(index) = self.project_row_at(column, row) else {
            return false;
        };
        let (target, items) = match self.project_rows().get(index).copied() {
            Some(ProjectListRow::Project { root, .. }) => (
                MenuTarget::Project(self.agents[root].project_key().to_string()),
                vec![
                    MenuItem::AddAgent,
                    MenuItem::Info,
                    MenuItem::Fold,
                    MenuItem::RemoveProject,
                ],
            ),
            Some(row @ ProjectListRow::EmptyProject { .. }) => {
                let Some(key) = self.header_key(row) else {
                    return false;
                };
                (
                    MenuTarget::Project(key),
                    vec![MenuItem::AddAgent, MenuItem::Info, MenuItem::RemoveProject],
                )
            }
            Some(ProjectListRow::Agent { index }) => {
                let items = if self.agent_has_spawns(index) {
                    vec![
                        MenuItem::Open,
                        MenuItem::Fold,
                        MenuItem::Archive,
                        MenuItem::RemoveProject,
                    ]
                } else {
                    vec![MenuItem::Open, MenuItem::Archive, MenuItem::RemoveProject]
                };
                (MenuTarget::Agent(self.agents[index].id.clone()), items)
            }
            _ => return false,
        };
        let items = self.with_workspace_item(&target, items);
        self.focus = PanelFocus::Swarm;
        self.selected_agent = index;
        self.agents_scroll_detached = false;
        self.context_menu = Some(ContextMenu::new(target, items, Position::new(column, row)));
        true
    }

    /// A right click on an Archive row: highlight it and offer to bring
    /// it back.
    fn open_archive_menu(&mut self, column: u16, row: u16) -> bool {
        let Some(index) = self.archived_row_at(column, row) else {
            return false;
        };
        let Some(id) = self.archived.get(index).map(|a| a.id.clone()) else {
            return false;
        };
        self.focus = PanelFocus::Swarm;
        self.select_archived_row(index);
        self.context_menu = Some(ContextMenu::new(
            MenuTarget::Archived(id),
            vec![MenuItem::Restore],
            Position::new(column, row),
        ));
        true
    }

    /// A right click on the CHANNELS tab: open the menu of the channel
    /// under the pointer, or of the open channel.
    fn open_channel_menu(&mut self, column: u16, row: u16) -> bool {
        let Some(id) = self.channel_menu_target_at(column, row) else {
            return false;
        };
        self.open_menu_of_channel(id, column, row)
    }

    /// A right click on the side panel's INFO tab of the open channel:
    /// the open channel's menu, there. Returns whether a menu opened.
    pub(super) fn open_channel_side_menu(&mut self, column: u16, row: u16) -> bool {
        if self.side_tab() != super::SidePanelTab::ChannelInfo {
            return false;
        }
        let Some(id) = self.open_channel().map(|channel| channel.id.clone()) else {
            return false;
        };
        self.context_menu = None;
        self.open_menu_of_channel(id, column, row)
    }

    /// Open channel `id`'s menu at `column`, `row`.
    fn open_menu_of_channel(&mut self, id: String, column: u16, row: u16) -> bool {
        let items = vec![MenuItem::EditChannel, MenuItem::ArchiveChannel];
        self.focus = PanelFocus::Swarm;
        self.context_menu = Some(ContextMenu::new(
            MenuTarget::Channel(id),
            items,
            Position::new(column, row),
        ));
        true
    }

    /// Open the submenu beside the highlighted item, if that item has
    /// one, it isn't open yet and it would list something.
    fn open_submenu(&mut self) {
        let entries = match self.context_menu.as_ref() {
            Some(menu) => self.submenu_entries(menu),
            None => return,
        };
        let Some(menu) = self.context_menu.as_mut() else {
            return;
        };
        if menu.submenu.is_none() && menu.highlights_submenu_item() && !entries.is_empty() {
            menu.submenu = Some(Submenu {
                entries,
                selected: 0,
                area: None,
            });
        }
    }

    /// The pointer is on item `index`: highlight it, and open its submenu
    /// or close another item's.
    fn point_at_menu_item(&mut self, index: usize) {
        let Some(menu) = self.context_menu.as_mut() else {
            return;
        };
        menu.selected = index;
        if menu.highlights_submenu_item() {
            self.open_submenu();
        } else {
            menu.submenu = None;
        }
    }

    /// A pointer event while the menu is open. Returns whether the menu
    /// used it; a press outside closes the menu and is used up too.
    pub(super) fn context_menu_mouse(
        &mut self,
        kind: crossterm::event::MouseEventKind,
        column: u16,
        row: u16,
    ) -> bool {
        use crossterm::event::{MouseButton, MouseEventKind};
        let Some(menu) = self.context_menu.as_mut() else {
            return false;
        };
        let entry = menu
            .submenu
            .as_ref()
            .and_then(|submenu| submenu.entry_at(column, row));
        // The submenu may be drawn over the menu, so where it is it takes
        // the pointer, border included: the item hidden under it is none.
        let in_submenu = menu
            .submenu
            .as_ref()
            .is_some_and(|submenu| submenu.covers(column, row));
        let item = if in_submenu {
            None
        } else {
            menu.item_at(column, row)
        };
        let covers = menu.covers_any(column, row);
        match kind {
            MouseEventKind::Moved => {
                if let Some(index) = entry {
                    if let Some(submenu) = menu.submenu.as_mut() {
                        submenu.selected = index;
                    }
                } else if let Some(index) = item {
                    self.point_at_menu_item(index);
                }
                covers
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = entry {
                    if let Some(submenu) = menu.submenu.as_mut() {
                        submenu.selected = index;
                    }
                    self.run_submenu_entry();
                } else if let Some(index) = item {
                    self.point_at_menu_item(index);
                    let opens_submenu = self
                        .context_menu
                        .as_ref()
                        .is_some_and(ContextMenu::highlights_submenu_item);
                    if !opens_submenu {
                        self.run_context_menu_item();
                    }
                } else if !covers {
                    self.context_menu = None;
                }
                true
            }
            MouseEventKind::Down(_) => {
                if covers {
                    return true;
                }
                self.context_menu = None;
                false
            }
            MouseEventKind::Up(_) | MouseEventKind::Drag(_) => covers,
            _ => {
                self.context_menu = None;
                false
            }
        }
    }

    /// Keys while the menu is open. Returns whether the key was used; any
    /// other key closes the menu and goes on to where it would have.
    pub(super) fn handle_context_menu_key(&mut self, key: KeyEvent) -> bool {
        if !matches!(self.state, AppState::Idle | AppState::Processing) {
            self.context_menu = None;
            return false;
        }
        let Some(menu) = self.context_menu.as_mut() else {
            return false;
        };
        if let Some(submenu) = menu.submenu.as_mut() {
            if let Some(selected) = step(submenu.selected, submenu.entries.len(), key.code) {
                submenu.selected = selected;
                return true;
            }
            match key.code {
                KeyCode::Left | KeyCode::Esc => menu.submenu = None,
                KeyCode::Enter => self.run_submenu_entry(),
                KeyCode::Right => {}
                _ => {
                    self.context_menu = None;
                    return false;
                }
            }
            return true;
        }
        if let Some(selected) = step(menu.selected, menu.items.len(), key.code) {
            menu.selected = selected;
            return true;
        }
        match key.code {
            KeyCode::Enter | KeyCode::Right if menu.highlights_submenu_item() => {
                self.open_submenu();
            }
            KeyCode::Enter => self.run_context_menu_item(),
            KeyCode::Esc => self.context_menu = None,
            _ => {
                self.context_menu = None;
                return false;
            }
        }
        true
    }

    /// Close the menu and run the highlighted entry of its submenu: ask
    /// the host to move the project it was opened on (or its agent's
    /// project) to that workspace.
    fn run_submenu_entry(&mut self) {
        let Some(menu) = self.context_menu.take() else {
            return;
        };
        let Some(SubmenuEntry::Workspace(to)) = menu
            .submenu
            .and_then(|submenu| submenu.entries.into_iter().nth(submenu.selected))
        else {
            return;
        };
        let project = match menu.target {
            MenuTarget::Project(key) => Some(key),
            MenuTarget::Agent(id) => self.project_key_of_agent(&id),
            MenuTarget::Archived(_) | MenuTarget::Channel(_) => None,
        };
        if let Some(project) = project {
            self.workspace_move = Some(WorkspaceMove { project, to });
        }
    }

    /// Close the menu and run its highlighted item on its target.
    fn run_context_menu_item(&mut self) {
        let Some(menu) = self.context_menu.take() else {
            return;
        };
        let Some(&item) = menu.items.get(menu.selected) else {
            return;
        };
        match menu.target {
            MenuTarget::Archived(id) => {
                if item == MenuItem::Restore {
                    self.restore_agent(id);
                }
            }
            MenuTarget::Channel(id) => match item {
                MenuItem::EditChannel => self.start_edit_channel(&id),
                MenuItem::ArchiveChannel => self.request_archive_channel(id),
                MenuItem::AddAgent
                | MenuItem::Info
                | MenuItem::Fold
                | MenuItem::Open
                | MenuItem::Archive
                | MenuItem::MoveToWorkspace
                | MenuItem::RemoveProject
                | MenuItem::Restore => {}
            },
            MenuTarget::Project(key) => match item {
                MenuItem::AddAgent => self.add_agent_to_project(&key),
                MenuItem::Fold => {
                    if let Some(root) = self.project_root_of_key(&key) {
                        self.toggle_project(root);
                    }
                }
                MenuItem::Info => self.show_project_info(&key),
                MenuItem::RemoveProject => self.request_remove_project(key),
                // Opens its submenu instead of running.
                MenuItem::Open
                | MenuItem::Archive
                | MenuItem::MoveToWorkspace
                | MenuItem::Restore
                | MenuItem::EditChannel
                | MenuItem::ArchiveChannel => {}
            },
            MenuTarget::Agent(id) => {
                let Some(index) = self.agents.iter().position(|agent| agent.id == id) else {
                    return;
                };
                match item {
                    MenuItem::Open => self.request_agent_switch(id),
                    MenuItem::Fold => self.toggle_agent(index),
                    MenuItem::Archive => self.archive_agent(id),
                    MenuItem::RemoveProject => self.request_remove_project_of(&id),
                    MenuItem::Restore => self.restore_agent(id),
                    MenuItem::AddAgent
                    | MenuItem::Info
                    | MenuItem::MoveToWorkspace
                    | MenuItem::EditChannel
                    | MenuItem::ArchiveChannel => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{AgentSummary, UiCommand, UiEvent};
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use tokio::sync::mpsc;

    fn agent(id: &str, depth: usize) -> AgentSummary {
        AgentSummary {
            id: id.into(),
            name: id.to_uppercase(),
            depth,
            ..AgentSummary::default()
        }
    }

    fn app() -> (App, mpsc::Receiver<UiEvent>) {
        let (tx, rx) = mpsc::channel(16);
        let mut app = App::new();
        app.event_tx = Some(tx);
        app.process_command(UiCommand::SetAgents(vec![
            agent("aa", 0),
            agent("bb", 1),
            agent("cc", 0),
        ]));
        app.process_command(UiCommand::SetActiveAgent("bb".into()));
        (app, rx)
    }

    /// Draw the app 120 columns wide and `height` rows tall.
    fn draw(app: &mut App, height: u16) -> ratatui::buffer::Buffer {
        draw_sized(app, 120, height)
    }

    /// Draw the app `width` columns wide and `height` rows tall.
    fn draw_sized(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
        use ratatui::{backend::TestBackend, Terminal};
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let theme = crate::Theme::cyber();
        terminal
            .draw(|frame| crate::renderer::render(frame, app, &theme))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    /// Draw the app 120 columns wide and `height` rows tall, a line per row.
    fn screen(app: &mut App, height: u16) -> Vec<String> {
        screen_sized(app, 120, height)
    }

    /// Draw the app `width` columns wide and `height` rows tall, a line
    /// per row.
    fn screen_sized(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let buffer = draw_sized(app, width, height);
        let width = usize::from(buffer.area.width);
        buffer
            .content
            .chunks(width)
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
            .collect()
    }

    /// The cell of the first row whose text contains `name`.
    fn cell_of(rows: &[String], name: &str) -> (u16, u16) {
        let row = rows
            .iter()
            .position(|row| row.contains(name))
            .unwrap_or_else(|| panic!("no {name}:\n{}", rows.join("\n")));
        let line = &rows[row];
        let column = line[..line.find(name).unwrap_or(0)].chars().count();
        (u16::try_from(column).unwrap(), u16::try_from(row).unwrap())
    }

    fn press(app: &mut App, button: MouseButton, (column, row): (u16, u16)) {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(button),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
    }

    #[test]
    fn right_click_in_the_input_box_asks_for_a_paste() {
        let mut app = App::new();
        app.set_input_area(Rect::new(10, 20, 40, 3));
        press(&mut app, MouseButton::Right, (12, 21));
        assert!(app.take_paste_request());
        press(&mut app, MouseButton::Right, (12, 5));
        assert!(!app.take_paste_request(), "outside the box");
    }

    #[test]
    fn super_v_asks_for_a_paste() {
        let mut app = App::new();
        app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::SUPER));
        assert!(app.take_paste_request());
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn right_clicking_a_project_lists_its_actions() {
        let (mut app, _rx) = app();
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Right, cell_of(&rows, "CC"));
        let rows = screen(&mut app, 30);
        for label in ["Add agent", "Project info", "Collapse", "Remove project…"] {
            assert!(
                rows.iter().any(|row| row.contains(label)),
                "{label}:\n{}",
                rows.join("\n")
            );
        }
        assert_eq!(app.focus(), PanelFocus::Swarm);
    }

    #[test]
    fn the_item_under_the_pointer_and_the_row_clicked_are_lit() {
        let (mut app, _rx) = app();
        let rows = screen(&mut app, 30);
        let project = cell_of(&rows, "CC");
        press(&mut app, MouseButton::Right, project);
        let rows = screen(&mut app, 30);
        let (add, rename) = (cell_of(&rows, "Add agent"), cell_of(&rows, "Project info"));
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: rename.0,
            row: rename.1,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.context_menu().map(|menu| menu.selected), Some(1));

        let buffer = draw(&mut app, 30);
        let hover = Some(crate::Theme::cyber().colors.surface_hover);
        let area = app
            .context_menu()
            .and_then(|menu| menu.area)
            .expect("drawn");
        let row_end = area.right() - 2;
        assert_eq!(buffer[rename].style().bg, hover, "the item hovered");
        assert_eq!(
            buffer[(row_end, rename.1)].style().bg,
            hover,
            "across its row"
        );
        assert_ne!(buffer[add].style().bg, hover, "the other items");
        assert_eq!(buffer[project].style().bg, hover, "the project behind");
    }

    #[test]
    fn a_project_item_runs_on_the_project_clicked() {
        let (mut app, mut rx) = app();
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Right, cell_of(&rows, "CC"));
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Left, cell_of(&rows, "Add agent"));
        assert!(app.context_menu().is_none());
        assert!(matches!(rx.try_recv(), Ok(UiEvent::AddAgentToProject(id)) if id == "cc"));
    }

    #[test]
    fn right_clicking_an_agent_archives_it_from_the_keys() {
        let (mut app, mut rx) = app();
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Right, cell_of(&rows, "BB"));
        let menu = app.context_menu().expect("the menu opened");
        assert_eq!(menu.target, MenuTarget::Agent("bb".into()));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        assert!(app.context_menu().is_none());
        assert!(matches!(rx.try_recv(), Ok(UiEvent::ArchiveAgent(id)) if id == "bb"));
    }

    #[test]
    fn an_agent_with_spawns_folds_from_its_menu() {
        let (mut app, _rx) = app();
        // The project's header reads "AA" too; the agent is the next row.
        let agent_aa = |rows: &[String]| {
            let (column, row) = cell_of(rows, "AA");
            (column, row + 1)
        };
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Right, agent_aa(&rows));
        let menu = app.context_menu().expect("the menu opened");
        assert_eq!(menu.target, MenuTarget::Agent("aa".into()));
        assert!(menu.items.contains(&MenuItem::Fold));
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Left, cell_of(&rows, "Collapse"));
        assert!(app.agent_folded(0));
        assert!(!app
            .project_rows()
            .contains(&ProjectListRow::Agent { index: 1 }));

        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Right, agent_aa(&rows));
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Left, cell_of(&rows, "Expand"));
        assert!(!app.agent_folded(0));

        // A leaf has nothing to fold.
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Right, cell_of(&rows, "BB"));
        let menu = app.context_menu().expect("the menu opened");
        assert!(!menu.items.contains(&MenuItem::Fold));
    }

    /// The app with two archived agents, showing the Archive tab with the
    /// first opened over the second, and on screen.
    fn archive_app() -> (App, mpsc::Receiver<UiEvent>) {
        let (mut app, mut rx) = app();
        app.process_command(UiCommand::SetArchivedAgents(vec![
            agent("dd", 0),
            agent("ee", 1),
        ]));
        app.focus = PanelFocus::Swarm;
        app.show_agents_tab(AgentsTab::Archive);
        assert_eq!(app.agents_tab(), AgentsTab::Archive);
        assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "dd"));
        assert!(rx.try_recv().is_err());
        // The Archive opens folded; Space opens dd over ee.
        app.handle_key(key(KeyCode::Char(' ')));
        (app, rx)
    }

    #[test]
    fn right_clicking_an_archived_agent_unarchives_it() {
        let (mut app, mut rx) = archive_app();
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Right, cell_of(&rows, "EE"));
        let menu = app.context_menu().expect("the menu opened");
        assert_eq!(menu.target, MenuTarget::Archived("ee".into()));
        assert_eq!(menu.items, vec![MenuItem::Restore]);
        assert_eq!(app.selected_archived(), 1, "the row is highlighted");
        assert!(rx.try_recv().is_err(), "opening the menu runs nothing");

        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Left, cell_of(&rows, "Unarchive"));
        assert!(app.context_menu().is_none());
        assert!(matches!(rx.try_recv(), Ok(UiEvent::RestoreAgent(id)) if id == "ee"));
        assert_eq!(app.agents_tab(), AgentsTab::Projects);
    }

    #[test]
    fn the_archive_menu_unarchives_from_the_keys() {
        let (mut app, mut rx) = archive_app();
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Right, cell_of(&rows, "DD"));
        assert!(app.context_menu().is_some());
        app.handle_key(key(KeyCode::Enter));
        assert!(app.context_menu().is_none());
        assert!(matches!(rx.try_recv(), Ok(UiEvent::RestoreAgent(id)) if id == "dd"));
    }

    #[test]
    fn right_clicking_empty_archive_space_opens_no_menu() {
        let (mut app, mut rx) = archive_app();
        let rows = screen(&mut app, 30);
        let (column, row) = cell_of(&rows, "EE");
        // Several rows below the last archived agent, still in the panel.
        press(&mut app, MouseButton::Right, (column, row + 4));
        assert!(app.context_menu().is_none());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn project_info_shows_the_folder_and_closes_on_any_key() {
        let (mut app, mut rx) = app();
        app.process_command(UiCommand::SetProjects(vec![
            crate::events::ProjectSummary {
                id: "aa".into(),
                name: "site".into(),
                folder: "/work/site".into(),
                folder_exists: true,
                ..Default::default()
            },
            crate::events::ProjectSummary {
                id: "cc".into(),
                name: "notes".into(),
                folder: "/work/notes".into(),
                folder_exists: false,
                created: "2026-11-19 14:00".into(),
                listed_agents: 1,
                archived_agents: 2,
            },
        ]));
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Right, cell_of(&rows, "CC"));
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Left, cell_of(&rows, "Project info"));
        assert!(rx.try_recv().is_err(), "nothing goes to the session");
        assert_eq!(app.state(), AppState::ShowingProjectInfo);
        assert_eq!(app.project_info().map(|p| p.id.as_str()), Some("cc"));

        let rows = screen(&mut app, 30);
        for text in [
            "NOTES",
            "/work/notes",
            "The folder is missing",
            "2026-11-19 14:00",
        ] {
            assert!(
                rows.iter().any(|row| row.contains(text)),
                "{text}:\n{}",
                rows.join("\n")
            );
        }
        app.handle_key(key(KeyCode::Char('x')));
        assert_eq!(app.state(), AppState::Idle);
        assert!(app.project_info().is_none());

        // `/project` shows the active agent's project.
        app.handle_command("/project");
        assert_eq!(app.project_info().map(|p| p.id.as_str()), Some("aa"));
    }

    #[test]
    fn esc_or_a_click_elsewhere_closes_the_menu() {
        let (mut app, mut rx) = app();
        let rows = screen(&mut app, 30);
        let project = cell_of(&rows, "CC");
        press(&mut app, MouseButton::Right, project);
        assert!(app.context_menu().is_some());
        app.handle_key(key(KeyCode::Esc));
        assert!(app.context_menu().is_none());

        press(&mut app, MouseButton::Right, project);
        screen(&mut app, 30);
        press(&mut app, MouseButton::Left, (110, 1));
        assert!(app.context_menu().is_none());
        assert!(rx.try_recv().is_err(), "closing runs nothing");
    }

    #[test]
    fn the_menu_stays_on_screen_near_the_bottom() {
        let (mut app, _rx) = app();
        let rows = screen(&mut app, 12);
        let at = cell_of(&rows, "BB");
        press(&mut app, MouseButton::Right, at);
        let rows = screen(&mut app, 12);
        let area = app.context_menu().and_then(|m| m.area).expect("drawn");
        assert!(area.bottom() <= 12, "{area:?}");
        assert!(
            area.bottom() <= at.1,
            "opens upward over the list: {area:?}"
        );
        assert!(rows.iter().any(|row| row.contains("Remove project…")));
    }

    /// [`app`] as the second of three workspaces of a host.
    fn workspace_app() -> (App, mpsc::Receiver<UiEvent>) {
        let (mut app, rx) = app();
        app.set_workspaces(vec!["SITE / AA".into(), "NOTES".into(), String::new()], 1);
        (app, rx)
    }

    fn hover(app: &mut App, (column, row): (u16, u16)) -> bool {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    fn submenu(app: &App) -> Option<&Submenu> {
        app.context_menu().and_then(|menu| menu.submenu.as_ref())
    }

    #[test]
    fn without_workspaces_the_menu_offers_no_move() {
        let (mut app, _rx) = app();
        let rows = screen(&mut app, 30);
        for name in ["CC", "BB"] {
            press(&mut app, MouseButton::Right, cell_of(&rows, name));
            let menu = app.context_menu().expect("the menu opened");
            assert!(!menu.items.contains(&MenuItem::MoveToWorkspace), "{name}");
        }
        assert!(app.take_workspace_move().is_none());
    }

    #[test]
    fn projects_and_agents_offer_a_move_before_removing() {
        let (mut app, _rx) = workspace_app();
        let rows = screen(&mut app, 30);
        for name in ["CC", "BB"] {
            press(&mut app, MouseButton::Right, cell_of(&rows, name));
            let items = &app.context_menu().expect("the menu opened").items;
            let remove = items.iter().position(|&i| i == MenuItem::RemoveProject);
            let at = items.iter().position(|&i| i == MenuItem::MoveToWorkspace);
            assert_eq!(at.map(|at| at + 1), remove, "{name}: {items:?}");
        }
        let rows = screen(&mut app, 30);
        let item = rows
            .iter()
            .find(|row| row.contains("Move to workspace"))
            .expect("drawn");
        assert!(item.contains('›'), "{item}");

        // A project with every agent archived, too.
        app.process_command(UiCommand::SetProjects(vec![
            crate::events::ProjectSummary {
                id: "ee".into(),
                name: "empty".into(),
                ..Default::default()
            },
        ]));
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Right, cell_of(&rows, "EMPTY"));
        let menu = app.context_menu().expect("the menu opened");
        assert_eq!(menu.target, MenuTarget::Project("ee".into()));
        assert!(menu.items.contains(&MenuItem::MoveToWorkspace));
    }

    #[test]
    fn the_archive_offers_no_move() {
        let (mut app, _rx) = archive_app();
        app.set_workspaces(vec!["A".into(), "B".into()], 0);
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Right, cell_of(&rows, "EE"));
        let menu = app.context_menu().expect("the menu opened");
        assert_eq!(menu.items, vec![MenuItem::Restore]);
    }

    #[test]
    fn the_keys_move_an_agents_project_to_another_workspace() {
        let (mut app, mut rx) = workspace_app();
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Right, cell_of(&rows, "BB"));
        // Open, Archive, then Move to workspace.
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        assert!(submenu(&app).is_none());
        app.handle_key(key(KeyCode::Right));
        let entries = &submenu(&app).expect("Right opens it").entries;
        assert_eq!(
            entries,
            &[
                SubmenuEntry::Workspace(WorkspaceTarget::Existing(0)),
                SubmenuEntry::Workspace(WorkspaceTarget::Existing(2)),
                SubmenuEntry::Workspace(WorkspaceTarget::New)
            ],
            "every workspace but this one, then a new one"
        );

        let rows = screen(&mut app, 30);
        for label in ["Workspace 1  SITE / AA", "Workspace 3", "New workspace"] {
            assert!(
                rows.iter().any(|row| row.contains(label)),
                "{label}:\n{}",
                rows.join("\n")
            );
        }
        assert!(!rows.iter().any(|row| row.contains("Workspace 2")));

        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        assert!(app.context_menu().is_none());
        assert_eq!(
            app.take_workspace_move(),
            Some(WorkspaceMove {
                project: "aa".into(),
                to: WorkspaceTarget::Existing(2),
            }),
            "the agent's whole project moves"
        );
        assert!(app.take_workspace_move().is_none(), "once");
        assert!(rx.try_recv().is_err(), "nothing goes to the session");
    }

    #[test]
    fn enter_opens_the_submenu_even_with_one_workspace() {
        let (mut app, mut rx) = app();
        app.set_workspaces(vec![String::new()], 0);
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Right, cell_of(&rows, "CC"));
        app.handle_key(key(KeyCode::End));
        app.handle_key(key(KeyCode::Up));
        app.handle_key(key(KeyCode::Enter));
        let entries = &submenu(&app).expect("Enter opens it").entries;
        assert_eq!(entries, &[SubmenuEntry::Workspace(WorkspaceTarget::New)]);
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(
            app.take_workspace_move(),
            Some(WorkspaceMove {
                project: "cc".into(),
                to: WorkspaceTarget::New,
            })
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn the_pointer_opens_the_submenu_and_picks_a_workspace() {
        let (mut app, mut rx) = workspace_app();
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Right, cell_of(&rows, "CC"));
        let rows = screen(&mut app, 30);
        let item = cell_of(&rows, "Move to workspace");
        assert!(hover(&mut app, item), "redraws");
        assert!(submenu(&app).is_some(), "hovering opens it");

        let rows = screen(&mut app, 30);
        let first = cell_of(&rows, "Workspace 1");
        assert_eq!(first.1, item.1, "its first entry is on the item's row");
        let new = cell_of(&rows, "New workspace");
        assert!(hover(&mut app, new), "the hover moves, so it redraws");
        assert_eq!(submenu(&app).map(|s| s.selected), Some(2));
        let buffer = draw(&mut app, 30);
        let lit = Some(crate::Theme::cyber().colors.surface_hover);
        assert_eq!(buffer[new].style().bg, lit);
        assert_ne!(buffer[first].style().bg, lit);

        // Another item closes it, and the item opens it again.
        let rows = screen(&mut app, 30);
        hover(&mut app, cell_of(&rows, "Project info"));
        assert!(submenu(&app).is_none());
        press(&mut app, MouseButton::Left, item);
        assert!(app.context_menu().is_some(), "a click on it runs nothing");
        assert!(submenu(&app).is_some(), "a click opens it");

        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Left, cell_of(&rows, "New workspace"));
        assert!(app.context_menu().is_none());
        assert_eq!(
            app.take_workspace_move(),
            Some(WorkspaceMove {
                project: "cc".into(),
                to: WorkspaceTarget::New,
            })
        );
        assert!(rx.try_recv().is_err(), "nothing goes to the session");
    }

    #[test]
    fn left_or_esc_closes_only_the_submenu() {
        let (mut app, mut rx) = workspace_app();
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Right, cell_of(&rows, "BB"));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        for close in [KeyCode::Left, KeyCode::Esc] {
            app.handle_key(key(KeyCode::Right));
            assert!(submenu(&app).is_some());
            app.handle_key(key(close));
            assert!(submenu(&app).is_none(), "{close:?}");
            let menu = app.context_menu().expect("the menu stays");
            assert_eq!(menu.items[menu.selected], MenuItem::MoveToWorkspace);
        }

        // A click outside both closes everything.
        app.handle_key(key(KeyCode::Right));
        screen(&mut app, 30);
        press(&mut app, MouseButton::Left, (110, 1));
        assert!(app.context_menu().is_none());
        assert!(app.take_workspace_move().is_none());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn the_submenu_stays_on_screen_near_the_edges() {
        let (mut app, _rx) = workspace_app();
        let rows = screen(&mut app, 12);
        press(&mut app, MouseButton::Right, cell_of(&rows, "BB"));
        app.handle_key(key(KeyCode::End));
        app.handle_key(key(KeyCode::Up));
        app.handle_key(key(KeyCode::Right));
        screen(&mut app, 12);
        let menu = app.context_menu().and_then(|m| m.area).expect("drawn");
        let sub = submenu(&app).and_then(|s| s.area).expect("drawn");
        assert!(sub.bottom() <= 12, "{sub:?}");
        assert_eq!(sub.x, menu.right(), "right of the menu");

        // Opened near the right edge, it goes left of the menu.
        if let Some(open) = app.context_menu.as_mut() {
            open.anchor = Position::new(118, 2);
        }
        screen(&mut app, 30);
        let menu = app.context_menu().and_then(|m| m.area).expect("drawn");
        let sub = submenu(&app).and_then(|s| s.area).expect("drawn");
        assert!(menu.right() <= 120, "{menu:?}");
        assert_eq!(sub.right(), menu.x, "left of the menu: {sub:?} {menu:?}");
        assert!(sub.bottom() <= 30);
    }

    #[test]
    fn new_workspaces_refresh_an_open_submenu() {
        let (mut app, _rx) = workspace_app();
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Right, cell_of(&rows, "CC"));
        app.handle_key(key(KeyCode::End));
        app.handle_key(key(KeyCode::Up));
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::End));
        app.set_workspaces(vec!["A".into(), "B".into()], 0);
        let open = submenu(&app).expect("still open");
        assert_eq!(
            open.entries,
            [
                SubmenuEntry::Workspace(WorkspaceTarget::Existing(1)),
                SubmenuEntry::Workspace(WorkspaceTarget::New)
            ]
        );
        assert_eq!(open.selected, 1, "kept in range");

        // No workspaces any more: nothing to move to.
        app.set_workspaces(Vec::new(), 0);
        let menu = app.context_menu().expect("the menu stays");
        assert!(menu.submenu.is_none());
        assert!(!menu.items.contains(&MenuItem::MoveToWorkspace));
    }

    /// [`app`] with only the project of `aa` listed.
    fn one_project_app() -> (App, mpsc::Receiver<UiEvent>) {
        let (mut app, rx) = app();
        app.process_command(UiCommand::SetAgents(vec![agent("aa", 0), agent("bb", 1)]));
        (app, rx)
    }

    /// A project, listed with every agent archived.
    fn empty_project(id: &str) -> UiCommand {
        UiCommand::SetProjects(vec![crate::events::ProjectSummary {
            id: id.into(),
            name: id.into(),
            ..Default::default()
        }])
    }

    #[test]
    fn the_only_project_listed_is_offered_no_new_workspace() {
        let (mut app, _rx) = one_project_app();
        app.set_workspaces(vec!["A".into(), "B".into()], 0);
        let open_submenu = |app: &mut App| {
            let rows = screen(app, 30);
            press(app, MouseButton::Right, cell_of(&rows, "BB"));
            // Open, Archive, Move to workspace, Remove project…
            app.handle_key(key(KeyCode::End));
            app.handle_key(key(KeyCode::Up));
            app.handle_key(key(KeyCode::Right));
            submenu(app).expect("Right opens it").entries.clone()
        };
        assert_eq!(
            open_submenu(&mut app),
            [SubmenuEntry::Workspace(WorkspaceTarget::Existing(1))]
        );
        let rows = screen(&mut app, 30);
        assert!(rows.iter().any(|row| row.contains("Workspace 2")));
        assert!(!rows.iter().any(|row| row.contains("New workspace")));
        app.handle_key(key(KeyCode::Esc));
        app.handle_key(key(KeyCode::Esc));

        // A project with every agent archived is listed too.
        app.process_command(empty_project("ee"));
        assert_eq!(
            open_submenu(&mut app),
            [
                SubmenuEntry::Workspace(WorkspaceTarget::Existing(1)),
                SubmenuEntry::Workspace(WorkspaceTarget::New)
            ]
        );
    }

    #[test]
    fn one_project_and_one_workspace_offer_no_move() {
        let (mut app, _rx) = one_project_app();
        app.set_workspaces(vec![String::new()], 0);
        let rows = screen(&mut app, 30);
        // The project's header, then its agents.
        for name in ["AA", "BB"] {
            press(&mut app, MouseButton::Right, cell_of(&rows, name));
            let menu = app.context_menu().expect("the menu opened");
            assert!(!menu.items.contains(&MenuItem::MoveToWorkspace), "{name}");
            assert!(menu.items.contains(&MenuItem::RemoveProject), "{name}");
        }
        app.handle_key(key(KeyCode::Esc));

        // A second project gives the first one somewhere to go.
        app.process_command(empty_project("ee"));
        let rows = screen(&mut app, 30);
        press(&mut app, MouseButton::Right, cell_of(&rows, "BB"));
        let menu = app.context_menu().expect("the menu opened");
        assert!(menu.items.contains(&MenuItem::MoveToWorkspace));
    }

    /// Columns of a screen too narrow for the submenu on either side of
    /// the menu, so it is drawn over it.
    const NARROW: u16 = 40;

    /// Open the submenu of agent BB's menu on a [`NARROW`] screen and draw
    /// it. Returns the menu's and the submenu's areas, which overlap.
    fn open_submenu_over_the_menu(app: &mut App) -> (Rect, Rect) {
        let rows = screen_sized(app, NARROW, 30);
        // BB's name is cut off this narrow; it is the row under its
        // parent AA, which has the fold mark.
        let (column, parent) = cell_of(&rows, "▾");
        press(app, MouseButton::Right, (column, parent + 1));
        let menu = app.context_menu().expect("the menu opened");
        assert_eq!(menu.target, MenuTarget::Agent("bb".into()));
        // Open, Archive, Move to workspace, Remove project…
        app.handle_key(key(KeyCode::End));
        app.handle_key(key(KeyCode::Up));
        app.handle_key(key(KeyCode::Right));
        let rows = screen_sized(app, NARROW, 30);
        let menu = app.context_menu().and_then(|m| m.area).expect("drawn");
        let sub = submenu(app).and_then(|s| s.area).expect("drawn");
        assert!(
            sub.intersects(menu),
            "over the menu: {sub:?} {menu:?}\n{}",
            rows.join("\n")
        );
        (menu, sub)
    }

    /// The main item drawn under a cell the submenu covers.
    fn item_under(app: &App, (column, row): (u16, u16)) -> Option<MenuItem> {
        let menu = app.context_menu()?;
        menu.items.get(menu.item_at(column, row)?).copied()
    }

    /// The menu is open on BB with its submenu, and nothing ran.
    fn assert_still_open(app: &App, rx: &mut mpsc::Receiver<UiEvent>, what: &str) {
        let menu = app.context_menu().expect("the menu stays");
        assert_eq!(
            menu.items[menu.selected],
            MenuItem::MoveToWorkspace,
            "{what}"
        );
        assert!(menu.submenu.is_some(), "{what}: the submenu stays");
        assert!(rx.try_recv().is_err(), "{what}: nothing runs");
        assert_eq!(app.state(), AppState::Idle, "{what}");
        assert!(app.workspace_move.is_none(), "{what}");
    }

    #[test]
    fn the_submenus_top_border_over_the_menu_runs_nothing() {
        let (mut app, mut rx) = workspace_app();
        let (_, sub) = open_submenu_over_the_menu(&mut app);
        let border = (sub.x + 1, sub.y);
        assert_eq!(
            item_under(&app, border),
            Some(MenuItem::Archive),
            "the border hides the item above"
        );

        press(&mut app, MouseButton::Left, border);
        assert_still_open(&app, &mut rx, "click");
        hover(&mut app, border);
        assert_still_open(&app, &mut rx, "hover");
    }

    #[test]
    fn the_submenus_bottom_border_over_the_menu_runs_nothing() {
        // One workspace to go to: the bottom border is on the next item.
        let (mut app, mut rx) = one_project_app();
        app.set_workspaces(vec!["A".into(), "B".into()], 0);
        let (_, sub) = open_submenu_over_the_menu(&mut app);
        let border = (sub.x + 1, sub.bottom() - 1);
        assert_eq!(item_under(&app, border), Some(MenuItem::RemoveProject));

        hover(&mut app, border);
        assert_still_open(&app, &mut rx, "hover");
        press(&mut app, MouseButton::Left, border);
        assert_still_open(&app, &mut rx, "click");
    }

    #[test]
    fn an_entry_over_the_menu_still_picks_its_workspace() {
        let (mut app, _rx) = workspace_app();
        let (_, sub) = open_submenu_over_the_menu(&mut app);
        // The second entry sits on "Remove project…".
        let entry = (sub.x + 1, sub.y + 2);
        assert_eq!(item_under(&app, entry), Some(MenuItem::RemoveProject));
        hover(&mut app, entry);
        assert_eq!(submenu(&app).map(|s| s.selected), Some(1));
        press(&mut app, MouseButton::Left, entry);
        assert!(app.context_menu().is_none());
        assert_eq!(
            app.take_workspace_move(),
            Some(WorkspaceMove {
                project: "aa".into(),
                to: WorkspaceTarget::Existing(2),
            })
        );
        assert_eq!(app.state(), AppState::Idle, "no removal asked");
    }
}
