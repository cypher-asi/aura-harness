//! The right-hand panel's tabs, and its Processes list: everything agents
//! and the user have running, with the agent that started each one.

use super::{contains, tab_at, App, DetailView, HoverTarget, NotificationType};
use crate::app::file_view::FileView;
use crate::events::{ProcessSummary, UiEvent};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
pub use side_tab::SidePanelTab;

/// Rows PageUp/PageDown move the highlight in every side-panel tab.
pub(super) const SIDE_PAGE: isize = 10;

impl App {
    /// Which view the panel below the Record panel shows.
    #[must_use]
    pub const fn side_tab(&self) -> SidePanelTab {
        self.side.tab
    }

    /// Rows of the Processes tab, in display order.
    #[must_use]
    pub fn processes(&self) -> &[ProcessSummary] {
        &self.side.processes.items
    }

    /// Highlighted row of the Processes tab.
    #[must_use]
    pub const fn selected_process(&self) -> usize {
        self.side.processes.selected
    }

    /// Record where the side panel's header bar was drawn this frame.
    pub fn set_side_tabs_area(&mut self, area: Rect) {
        self.areas.side_tabs = Some(area);
    }

    /// Record where that bar drew HOST's icon this frame, if it did.
    pub fn set_side_icon_tab_area(&mut self, area: Option<Rect>) {
        self.areas.side_icon_tab = area;
    }

    /// Record where the process rows were drawn this frame and which row
    /// is at the top.
    pub fn set_process_rows_area(&mut self, area: Rect, scroll: usize) {
        self.areas.process_rows = Some(area);
        self.areas.processes_scroll = scroll;
    }

    /// Keep the highlighted process inside a list `visible_rows` tall,
    /// returning the first row to show.
    pub fn sync_processes_scroll(&mut self, visible_rows: usize) -> usize {
        self.side.processes.follow_selection(visible_rows);
        self.side.processes.scroll
    }

    /// Show `tab`. A channel's tabs are only shown while a channel is
    /// open, and the agent app's only while none is; asked for in the
    /// wrong one, nothing changes.
    pub(super) fn show_side_tab(&mut self, tab: SidePanelTab) {
        if tab.is_channel_tab() != self.channel_open() {
            return;
        }
        self.side.tab = tab;
        self.pending.kill = None;
    }

    /// Replace the list, keeping the highlight on the same process when it
    /// is still listed, and refreshing an open process detail view.
    pub(super) fn set_processes(&mut self, processes: Vec<ProcessSummary>) {
        self.side.all_processes = processes;
        self.reshow_processes();
        self.absorb_members();
        if let Some(DetailView::Process(open)) = self.detail.view.as_mut() {
            if let Some(fresh) = self.side.all_processes.iter().find(|p| p.id == open.id) {
                **open = fresh.clone();
            }
        }
    }

    /// List the processes the side panel's scope lets through, keeping
    /// the highlight on the same process when it is still listed.
    pub(super) fn reshow_processes(&mut self) {
        let selected_id = self.side.processes.selected_item().map(|p| p.id.clone());
        self.side.processes.items = self
            .side
            .all_processes
            .iter()
            .filter(|process| {
                self.side
                    .process_filter
                    .as_ref()
                    .is_none_or(|agents| super::side_scope::process_in_scope(process, agents))
            })
            .cloned()
            .collect();
        self.side.processes.selected = selected_id
            .and_then(|id| self.side.processes.items.iter().position(|p| p.id == id))
            .unwrap_or(self.side.processes.selected)
            .min(self.side.processes.len().saturating_sub(1));
    }

    /// Processes of agents the side panel's scope leaves out.
    #[must_use]
    pub fn hidden_processes(&self) -> usize {
        self.side.all_processes.len() - self.side.processes.len()
    }

    pub(crate) fn selected_process_data(&self) -> Option<&ProcessSummary> {
        self.side.processes.selected_item()
    }

    fn select_process(&mut self, index: usize) {
        self.side.processes.selected = index.min(self.side.processes.len().saturating_sub(1));
    }

    fn move_process_selection(&mut self, delta: isize) {
        self.select_process(self.side.processes.selected.saturating_add_signed(delta));
    }

    pub(super) fn scroll_processes(&mut self, delta: isize) {
        self.move_process_selection(delta);
    }

    /// Navigation keys of the Processes tab.
    pub(super) fn handle_processes_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.move_process_selection(-1),
            KeyCode::Down => self.move_process_selection(1),
            KeyCode::PageUp => self.move_process_selection(-SIDE_PAGE),
            KeyCode::PageDown => self.move_process_selection(SIDE_PAGE),
            KeyCode::Home => self.select_process(0),
            KeyCode::End => self.select_process(usize::MAX),
            KeyCode::Enter => self.open_selected_process(),
            _ => {}
        }
    }

    /// Shift+Left/Right switches the side panel's tab from either tab.
    pub(super) fn handle_side_tab_key(&mut self, key: KeyEvent) -> bool {
        if !key.modifiers.contains(KeyModifiers::SHIFT) {
            return false;
        }
        match key.code {
            KeyCode::Left => self.show_side_tab(self.side.tab.prev(self.channel_open())),
            KeyCode::Right => self.show_side_tab(self.side.tab.next(self.channel_open())),
            _ => return false,
        }
        true
    }

    /// Keys of the side panel's lists that would otherwise go to the input,
    /// used while it is empty: Home/End jump, Space folds in the trees, and
    /// each tab's letters (`[`/`]`/`o`/`r` on Repo, `o`/`k`/`l` on
    /// Processes, `g` on Trace, `p`/`t` on Tasks).
    pub(super) fn handle_side_list_shortcut(&mut self, key: KeyEvent) -> bool {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return false;
        }
        match (self.side.tab, key.code) {
            (_, KeyCode::Home | KeyCode::End) => {
                let index = if key.code == KeyCode::End {
                    usize::MAX
                } else {
                    0
                };
                match self.side.tab {
                    SidePanelTab::Sandbox => self.sandbox.select(index),
                    SidePanelTab::Repo => self.select_repo_row(index),
                    SidePanelTab::Processes => self.select_process(index),
                    SidePanelTab::Host => {
                        self.host_key(key);
                    }
                    SidePanelTab::Trace => self.trace_feed.select(index),
                    SidePanelTab::Tasks => self.task_list.select(index),
                    SidePanelTab::ChannelMembers => self.select_channel_member(index),
                    SidePanelTab::ChannelInfo => self.handle_channel_info_key(key),
                    SidePanelTab::ChannelQueue => self.handle_channel_queue_key(key),
                }
            }
            (SidePanelTab::Sandbox, KeyCode::Char(' ')) => self.sandbox.toggle_selected(),
            (SidePanelTab::Repo, KeyCode::Char(' ')) => self.toggle_repo_changes(),
            (SidePanelTab::ChannelMembers, KeyCode::Char(' ')) => {
                self.toggle_channel_member(self.selected_channel_member());
            }
            (SidePanelTab::Repo, KeyCode::Char('[')) => self.step_repo_filter(false),
            (SidePanelTab::Repo, KeyCode::Char(']')) => self.step_repo_filter(true),
            (SidePanelTab::Repo, KeyCode::Char('o')) => self.open_selected_repo_page(),
            (SidePanelTab::Repo, KeyCode::Char('r')) => self.request_repo(true),
            (SidePanelTab::Tasks, _) => return self.handle_tasks_shortcut(key),
            (SidePanelTab::Processes, KeyCode::Char('o')) => self.jump_to_selected_owner(),
            (SidePanelTab::Processes, KeyCode::Char('k') | KeyCode::Delete) => {
                self.request_kill_selected();
            }
            (SidePanelTab::Processes, KeyCode::Char('l')) => self.open_selected_log(),
            (SidePanelTab::Trace, KeyCode::Char('g')) => self.open_selected_trace_in_graph(),
            _ => return false,
        }
        true
    }

    pub(super) fn open_selected_process(&mut self) {
        if let Some(process) = self.selected_process_data().cloned() {
            self.open_detail(DetailView::Process(Box::new(process)));
        }
    }

    /// Show the agent that started the highlighted process in the chat.
    fn jump_to_selected_owner(&mut self) {
        let Some(process) = self.selected_process_data() else {
            return;
        };
        let target = if process.jump_agent_id.is_empty() {
            process.owner_agent_id.clone()
        } else {
            process.jump_agent_id.clone()
        };
        if target.is_empty() {
            return;
        }
        if target == self.active_agent_id {
            self.notifications.current = Some((
                format!("{} is already open", process.owner_name),
                NotificationType::Success,
            ));
            return;
        }
        self.request_agent_switch(target);
    }

    /// Open a background command's output in the file view.
    fn open_selected_log(&mut self) {
        let Some(path) = self
            .selected_process_data()
            .and_then(|p| p.log_path.clone())
        else {
            self.notifications.current = Some((
                "Only background commands have a log".to_string(),
                NotificationType::Warning,
            ));
            return;
        };
        let view = FileView::load(&path, path.parent());
        self.open_detail(DetailView::File(Box::new(view)));
    }

    /// Ask to confirm killing the highlighted process.
    fn request_kill_selected(&mut self) {
        let Some(process) = self.selected_process_data().cloned() else {
            return;
        };
        if !process.status.is_live() {
            self.notifications.current = Some((
                format!("{} already {}", process.label, process.status.label()),
                NotificationType::Warning,
            ));
            return;
        }
        if !process.killable {
            self.notifications.current = Some((
                format!("{} can't be killed from here", process.label),
                NotificationType::Warning,
            ));
            return;
        }
        self.notifications.current = Some((
            format!(
                "Kill {} {}? Press y to confirm",
                process.kind.label(),
                process.label
            ),
            NotificationType::Warning,
        ));
        self.pending.kill = Some(process.id);
    }

    /// Resolve a pending kill with `key`. Returns `false` when nothing was
    /// pending.
    pub(super) fn handle_pending_kill(&mut self, key: KeyEvent) -> bool {
        let Some(id) = self.pending.kill.take() else {
            return false;
        };
        if matches!(key.code, KeyCode::Char('y' | 'Y')) {
            if let Some(tx) = &self.link.event_tx {
                let _ = tx.try_send(UiEvent::KillProcess(id));
            }
        }
        true
    }

    /// Clicking a tab shows it. Clicking a row highlights it and opens it
    /// over the chat panel at once, the same in every tab: a file, a
    /// process's details, a trace step, a plan (at the clicked todo), a
    /// task's agent; a REPO row's diff goes in the chat. Directories have
    /// nothing to open, so a click folds them; plans and tasks fold when
    /// their fold glyph is clicked.
    pub(super) fn click_side_panel(&mut self, column: u16, row: u16) {
        if self.side_scope_at(column, row) {
            self.toggle_side_scope();
            return;
        }
        if contains(self.areas.side_expand, column, row) {
            self.toggle_side_expanded();
            self.clear_selection();
            return;
        }
        if let Some(tab) = self.side_tab_at(column, row) {
            self.show_side_tab(tab);
            return;
        }
        match self.side.tab {
            SidePanelTab::Sandbox => {
                if let Some(index) = self.sandbox_row_at(column, row) {
                    self.sandbox.select(index);
                    self.activate_selected_sandbox_row();
                }
            }
            SidePanelTab::Repo => self.click_repo(column, row),
            SidePanelTab::Processes => {
                if let Some(index) = self.process_row_at(column, row) {
                    self.select_process(index);
                    self.open_selected_process();
                }
            }
            SidePanelTab::Host => {}
            SidePanelTab::Trace => {
                if let Some(index) = self.trace_row_at(column, row) {
                    self.trace_feed.select(index);
                    self.open_selected_trace_entry();
                }
            }
            SidePanelTab::Tasks => self.click_task_row(column, row),
            SidePanelTab::ChannelMembers => self.click_channel_member(column, row),
            SidePanelTab::ChannelInfo | SidePanelTab::ChannelQueue => {}
        }
    }

    /// The tab whose label or icon is at a screen cell.
    fn side_tab_at(&self, column: u16, row: u16) -> Option<SidePanelTab> {
        let channel_open = self.channel_open();
        if contains(self.areas.side_icon_tab, column, row) {
            return SidePanelTab::icon_tab(channel_open);
        }
        tab_at(self.areas.side_tabs, column, row, |width| {
            SidePanelTab::layout(width, channel_open)
        })
    }

    pub(super) fn sandbox_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let rows = self
            .areas
            .sandbox_rows
            .filter(|&rows| contains(Some(rows), column, row))?;
        let index = self.areas.sandbox_scroll + usize::from(row - rows.y);
        (index < self.sandbox.rows().len()).then_some(index)
    }

    fn trace_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let rows = self
            .areas
            .trace_rows
            .filter(|&rows| contains(Some(rows), column, row))?;
        let index = self.areas.trace_scroll + usize::from(row - rows.y);
        (index < self.trace_feed.len()).then_some(index)
    }

    fn process_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let rows = self
            .areas
            .process_rows
            .filter(|&rows| contains(Some(rows), column, row))?;
        let index = self.areas.processes_scroll + usize::from(row - rows.y);
        (index < self.side.processes.len()).then_some(index)
    }

    /// Clickable item of the side panel at a screen cell: a tab label, or
    /// a row of the open tab.
    pub(super) fn hovered_side_panel(&self, column: u16, row: u16) -> Option<HoverTarget> {
        if self.side_scope_at(column, row) {
            return Some(HoverTarget::SideScope);
        }
        if contains(self.areas.side_expand, column, row) {
            return Some(HoverTarget::ExpandSide);
        }
        if let Some(tab) = self.side_tab_at(column, row) {
            return Some(HoverTarget::SideTab(tab));
        }
        match self.side.tab {
            SidePanelTab::Sandbox => self
                .sandbox_row_at(column, row)
                .map(HoverTarget::SandboxRow),
            SidePanelTab::Repo => self.hovered_repo(column, row),
            SidePanelTab::Processes => self
                .process_row_at(column, row)
                .map(HoverTarget::ProcessRow),
            SidePanelTab::Host => None,
            SidePanelTab::Trace => self.trace_row_at(column, row).map(HoverTarget::TraceRow),
            SidePanelTab::Tasks => self.task_row_at(column, row).map(HoverTarget::TaskRow),
            SidePanelTab::ChannelMembers => self
                .channel_member_row_at(column, row)
                .map(HoverTarget::ChannelMemberRow),
            SidePanelTab::ChannelInfo | SidePanelTab::ChannelQueue => None,
        }
    }
}

/// Whether `column` falls on the icon of a side-panel row drawn at
/// `depth` in rows starting at `rows_x`: the icon column of
/// `components::list_row`, the row's inset and two per level in.
pub(super) fn on_row_icon(column: u16, rows_x: u16, depth: usize) -> bool {
    let start = crate::components::ROW_INSET + 2 * depth;
    let offset = usize::from(column.saturating_sub(rows_x));
    column >= rows_x && (start..start + crate::animation::AURA_WIDTH + 1).contains(&offset)
}

mod side_tab;
#[cfg(test)]
mod tests;
