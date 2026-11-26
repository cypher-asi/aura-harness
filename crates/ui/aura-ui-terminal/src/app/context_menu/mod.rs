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

use super::{contains, App, AppState, PanelFocus, ProjectListRow};
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

    /// A right click on the PROJECTS list: highlight the project or agent
    /// under the pointer and open its menu there. Returns whether a menu
    /// opened.
    pub(super) fn open_project_menu(&mut self, column: u16, row: u16) -> bool {
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
    pub(super) fn open_archive_menu(&mut self, column: u16, row: u16) -> bool {
        let Some(index) = self.archived_row_at(column, row) else {
            return false;
        };
        let Some(id) = self.archived.items.get(index).map(|a| a.id.clone()) else {
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
    pub(super) fn open_channel_menu(&mut self, column: u16, row: u16) -> bool {
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
mod tests;
