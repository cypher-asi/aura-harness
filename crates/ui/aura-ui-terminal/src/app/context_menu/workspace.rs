//! The workspaces the host has, and "Move to workspace".

use super::{
    ContextMenu, MenuItem, MenuTarget, SubmenuEntry, WorkspaceMove, WorkspaceTarget, Workspaces,
};
use crate::app::{App, ProjectListRow};

impl App {
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
    pub(super) fn submenu_entries(&self, menu: &ContextMenu) -> Vec<SubmenuEntry> {
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
    /// `items` with "Move to workspace" before "Remove project…", when
    /// the host has a workspace `target` can move to.
    pub(super) fn with_workspace_item(
        &self,
        target: &MenuTarget,
        mut items: Vec<MenuItem>,
    ) -> Vec<MenuItem> {
        if !self.workspace_targets(target).is_empty() {
            let at = items
                .iter()
                .position(|&item| item == MenuItem::RemoveProject)
                .unwrap_or(items.len());
            items.insert(at, MenuItem::MoveToWorkspace);
        }
        items
    }
}
