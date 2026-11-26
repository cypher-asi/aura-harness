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
//! [`App::set_workspaces`](super::App::set_workspaces)), projects and agents also get "Move to
//! workspace", whose submenu lists the other workspaces and a new one.
//! A new one is left out for the only project listed, which has a
//! workspace of its own already, and the item itself when that leaves
//! nothing to pick.
//! Picking one queues a [`WorkspaceMove`] for the host to take with
//! [`App::take_workspace_move`](super::App::take_workspace_move); the session hears nothing of it, since
//! workspaces are the host's alone. An agent moves with its whole project,
//! because a workspace holds projects, not single agents.
//!
//! On the CHANNELS tab the menu opens on a channel row of the list, or
//! anywhere in the open channel's INFO side tab, and offers "Edit
//! channel…", which opens the channel form on it (title, description and
//! project), and "Archive channel…", which asks for `y` first.

mod input;
mod open;
mod run;
mod workspace;

use super::contains;
use crossterm::event::KeyCode;
use ratatui::layout::{Position, Rect};

/// Where a project is to go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceTarget {
    /// The workspace at this index of the labels given to
    /// [`App::set_workspaces`](super::App::set_workspaces)
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

/// What lies under the pointer of an open menu.
#[derive(Debug, Clone, Copy)]
struct MenuHit {
    /// The submenu entry there
    entry: Option<usize>,
    /// The menu item there, unless the submenu covers it
    item: Option<usize>,
    /// Whether the menu or its submenu is drawn there
    covers: bool,
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

    /// What lies under a screen cell.
    fn hit(&self, column: u16, row: u16) -> MenuHit {
        let entry = self
            .submenu
            .as_ref()
            .and_then(|submenu| submenu.entry_at(column, row));
        // The submenu may be drawn over the menu, so where it is it takes
        // the pointer, border included: the item hidden under it is none.
        let in_submenu = self
            .submenu
            .as_ref()
            .is_some_and(|submenu| submenu.covers(column, row));
        let item = if in_submenu {
            None
        } else {
            self.item_at(column, row)
        };
        MenuHit {
            entry,
            item,
            covers: self.covers_any(column, row),
        }
    }

    fn highlights_submenu_item(&self) -> bool {
        self.items
            .get(self.selected)
            .is_some_and(|item| item.has_submenu())
    }
}

#[cfg(test)]
mod tests;
