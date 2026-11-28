//! Opening the menu, and what its items read.

use super::{ContextMenu, MenuItem, MenuTarget, Submenu};
use crate::app::{App, PanelFocus, ProjectListRow, SidePanelTab};
use ratatui::layout::{Position, Rect};

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
    /// What an item reads, for the target it was opened on.
    pub(crate) fn menu_label(&self, menu: &ContextMenu, item: MenuItem) -> &'static str {
        match item {
            MenuItem::AddAgent => "Add agent",
            MenuItem::Info => "Project info",
            MenuItem::Fold => {
                let folded = match &menu.target {
                    MenuTarget::Project(id) => self.project_list.collapsed_projects.contains(id),
                    MenuTarget::Agent(id) => self.project_list.collapsed_agents.contains(id),
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
    /// A right click on the PROJECTS list: highlight the project or agent
    /// under the pointer and open its menu there. Returns whether a menu
    /// opened.
    pub(in crate::app) fn open_project_menu(&mut self, column: u16, row: u16) -> bool {
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
        self.project_list.selected = index;
        self.panels
            .wheel
            .release(crate::app::wheel_views::WheelList::Projects);
        self.context_menu = Some(ContextMenu::new(target, items, Position::new(column, row)));
        true
    }

    /// A right click on an Archive row: highlight it and offer to bring
    /// it back.
    pub(in crate::app) fn open_archive_menu(&mut self, column: u16, row: u16) -> bool {
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
    pub(in crate::app) fn open_channel_menu(&mut self, column: u16, row: u16) -> bool {
        let Some(id) = self.channel_menu_target_at(column, row) else {
            return false;
        };
        self.open_menu_of_channel(id, column, row)
    }

    /// A right click on the side panel's INFO tab of the open channel:
    /// the open channel's menu, there. Returns whether a menu opened.
    pub(in crate::app) fn open_channel_side_menu(&mut self, column: u16, row: u16) -> bool {
        if self.side_tab() != SidePanelTab::ChannelInfo {
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
    pub(super) fn open_submenu(&mut self) {
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
}
