//! Running the item or submenu entry picked.

use super::{MenuItem, MenuTarget, SubmenuEntry, WorkspaceMove};
use crate::app::App;

impl App {
    /// Close the menu and run the highlighted entry of its submenu: ask
    /// the host to move the project it was opened on (or its agent's
    /// project) to that workspace.
    pub(super) fn run_submenu_entry(&mut self) {
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
            self.pending.workspace_move = Some(WorkspaceMove { project, to });
        }
    }

    /// Close the menu and run its highlighted item on its target.
    pub(super) fn run_context_menu_item(&mut self) {
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
            MenuTarget::Channel(id) => self.run_channel_item(item, id),
            MenuTarget::Project(key) => self.run_project_item(item, key),
            MenuTarget::Agent(id) => self.run_agent_item(item, id),
        }
    }

    fn run_channel_item(&mut self, item: MenuItem, id: String) {
        match item {
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
        }
    }

    fn run_project_item(&mut self, item: MenuItem, key: String) {
        match item {
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
        }
    }

    fn run_agent_item(&mut self, item: MenuItem, id: String) {
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
