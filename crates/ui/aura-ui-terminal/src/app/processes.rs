//! The right-hand panel's tabs, and its Processes list: everything agents
//! and the user have running, with the agent that started each one.

use super::{
    contains, follow_selection, tab_at, tab_bar::TabLayout, App, DetailView, HoverTarget,
    NotificationType,
};
use crate::app::file_view::FileView;
use crate::events::{ProcessSummary, UiEvent};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

/// Rows PageUp/PageDown move the highlight in every side-panel tab.
pub(super) const SIDE_PAGE: isize = 10;

/// Views of the right-hand panel below the Record panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidePanelTab {
    /// The open channel's members: its coordinator and the agents it
    /// spawned. Listed only while a channel is open, first.
    ChannelMembers,
    /// The open channel's description, coordinator and linked projects.
    /// Listed only while a channel is open.
    ChannelInfo,
    /// File tree of the active agent's workspace.
    #[default]
    Sandbox,
    /// Live agent turns, spawned agents and background commands.
    Processes,
    /// The host the agent on screen runs on, this machine or its cloud
    /// machine: resources, addresses, status and cost.
    Host,
    /// Live feed of recorded steps and harness decisions.
    Trace,
    /// Work agents did or are doing, grouped under the plans they work
    /// on, each plan with its todos and each task with what is done and
    /// left.
    Tasks,
}

impl SidePanelTab {
    /// The tabs listed while no channel is open: the agent app's.
    const OUTSIDE_CHANNEL: [Self; 5] = [
        Self::Sandbox,
        Self::Processes,
        Self::Host,
        Self::Trace,
        Self::Tasks,
    ];

    /// The tabs listed while a channel is open: the channel's own, and
    /// none of the agent app's.
    const IN_CHANNEL: [Self; 2] = [Self::ChannelMembers, Self::ChannelInfo];

    /// The tabs the header bar lists, in order, with a channel open or
    /// not.
    #[must_use]
    pub const fn tabs(channel_open: bool) -> &'static [Self] {
        if channel_open {
            &Self::IN_CHANNEL
        } else {
            &Self::OUTSIDE_CHANNEL
        }
    }

    /// Whether this is one of a channel's own tabs, listed only while a
    /// channel is open.
    #[must_use]
    pub const fn is_channel_tab(self) -> bool {
        matches!(self, Self::ChannelMembers | Self::ChannelInfo)
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ChannelMembers => "MEMBERS",
            Self::ChannelInfo => "INFO",
            Self::Sandbox => "SANDBOX",
            Self::Processes => "PROCESSES",
            Self::Host => "HOST",
            Self::Trace => "TRACE",
            Self::Tasks => "TASKS",
        }
    }

    /// Label for header bars too narrow for `label`.
    #[must_use]
    pub const fn short_label(self) -> &'static str {
        match self {
            Self::ChannelMembers => "MBR",
            Self::ChannelInfo => "INF",
            Self::Sandbox => "SBX",
            Self::Processes => "PRC",
            Self::Host => "HST",
            Self::Trace => "TRC",
            Self::Tasks => "TSK",
        }
    }

    /// The tabs' cells in a header bar `width` columns wide, as drawn and
    /// as clicks are mapped back, with a channel open or not.
    #[must_use]
    pub fn layout(width: u16, channel_open: bool) -> TabLayout<Self> {
        if channel_open {
            TabLayout::new(Self::IN_CHANNEL, Self::label, Self::short_label, width)
        } else {
            TabLayout::new(Self::OUTSIDE_CHANNEL, Self::label, Self::short_label, width)
        }
    }

    /// The tab after this one in [`Self::tabs`], wrapping around. A tab
    /// that is not listed goes to the first.
    pub(super) fn next(self, channel_open: bool) -> Self {
        let tabs = Self::tabs(channel_open);
        let next = tabs
            .iter()
            .position(|&tab| tab == self)
            .map_or(0, |at| (at + 1) % tabs.len());
        tabs.get(next).copied().unwrap_or_default()
    }

    /// The tab before this one in [`Self::tabs`], wrapping around. A tab
    /// that is not listed goes to the last.
    pub(super) fn prev(self, channel_open: bool) -> Self {
        let tabs = Self::tabs(channel_open);
        let prev = tabs
            .iter()
            .position(|&tab| tab == self)
            .map_or(tabs.len() - 1, |at| (at + tabs.len() - 1) % tabs.len());
        tabs.get(prev).copied().unwrap_or_default()
    }
}

impl App {
    /// Which view the panel below the Record panel shows.
    #[must_use]
    pub const fn side_tab(&self) -> SidePanelTab {
        self.side_tab
    }

    /// Rows of the Processes tab, in display order.
    #[must_use]
    pub fn processes(&self) -> &[ProcessSummary] {
        &self.processes
    }

    /// Highlighted row of the Processes tab.
    #[must_use]
    pub const fn selected_process(&self) -> usize {
        self.selected_process
    }

    /// Record where the side panel's header bar was drawn this frame.
    pub fn set_side_tabs_area(&mut self, area: Rect) {
        self.areas.side_tabs = Some(area);
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
        self.processes_scroll = follow_selection(
            self.processes_scroll,
            self.selected_process,
            visible_rows,
            self.processes.len(),
        );
        self.processes_scroll
    }

    /// Show `tab`. A channel's tabs are only shown while a channel is
    /// open, and the agent app's only while none is; asked for in the
    /// wrong one, nothing changes.
    pub(super) fn show_side_tab(&mut self, tab: SidePanelTab) {
        if tab.is_channel_tab() != self.channel_open() {
            return;
        }
        self.side_tab = tab;
        self.pending_kill = None;
    }

    /// Replace the list, keeping the highlight on the same process when it
    /// is still listed, and refreshing an open process detail view.
    pub(super) fn set_processes(&mut self, processes: Vec<ProcessSummary>) {
        self.all_processes = processes;
        self.reshow_processes();
        self.absorb_members();
        if let Some(DetailView::Process(open)) = self.detail.as_mut() {
            if let Some(fresh) = self.all_processes.iter().find(|p| p.id == open.id) {
                **open = fresh.clone();
            }
        }
    }

    /// List the processes the side panel's scope lets through, keeping
    /// the highlight on the same process when it is still listed.
    pub(super) fn reshow_processes(&mut self) {
        let selected_id = self
            .processes
            .get(self.selected_process)
            .map(|p| p.id.clone());
        self.processes = self
            .all_processes
            .iter()
            .filter(|process| {
                self.process_filter
                    .as_ref()
                    .is_none_or(|agents| super::side_scope::process_in_scope(process, agents))
            })
            .cloned()
            .collect();
        self.selected_process = selected_id
            .and_then(|id| self.processes.iter().position(|p| p.id == id))
            .unwrap_or(self.selected_process)
            .min(self.processes.len().saturating_sub(1));
    }

    /// Processes of agents the side panel's scope leaves out.
    #[must_use]
    pub fn hidden_processes(&self) -> usize {
        self.all_processes.len() - self.processes.len()
    }

    pub(crate) fn selected_process_data(&self) -> Option<&ProcessSummary> {
        self.processes.get(self.selected_process)
    }

    fn select_process(&mut self, index: usize) {
        self.selected_process = index.min(self.processes.len().saturating_sub(1));
    }

    fn move_process_selection(&mut self, delta: isize) {
        self.select_process(self.selected_process.saturating_add_signed(delta));
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
            KeyCode::Left => self.show_side_tab(self.side_tab.prev(self.channel_open())),
            KeyCode::Right => self.show_side_tab(self.side_tab.next(self.channel_open())),
            _ => return false,
        }
        true
    }

    /// Keys of the side panel's lists that would otherwise go to the input,
    /// used while it is empty: Home/End jump, Space folds in the trees, and
    /// each tab's letters (`o`/`k`/`l` on Processes, `g` on Trace, `p`/`t`
    /// on Tasks).
    pub(super) fn handle_side_list_shortcut(&mut self, key: KeyEvent) -> bool {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return false;
        }
        match (self.side_tab, key.code) {
            (_, KeyCode::Home | KeyCode::End) => {
                let index = if key.code == KeyCode::End {
                    usize::MAX
                } else {
                    0
                };
                match self.side_tab {
                    SidePanelTab::Sandbox => self.sandbox.select(index),
                    SidePanelTab::Processes => self.select_process(index),
                    SidePanelTab::Host => {
                        self.cloud.host_scroll = if index == 0 { 0 } else { usize::MAX / 2 };
                    }
                    SidePanelTab::Trace => self.trace_feed.select(index),
                    SidePanelTab::Tasks => self.task_list.select(index),
                    SidePanelTab::ChannelMembers => self.select_channel_member(index),
                    SidePanelTab::ChannelInfo => {}
                }
            }
            (SidePanelTab::Sandbox, KeyCode::Char(' ')) => self.sandbox.toggle_selected(),
            (SidePanelTab::Tasks, KeyCode::Char(' ')) => self.task_list.toggle_selected(),
            (SidePanelTab::Tasks, KeyCode::Char('p')) => self.open_selected_row_plan(),
            (SidePanelTab::Tasks, KeyCode::Char('t')) => self.jump_to_todo_task(),
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
            self.notification = Some((
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
            self.notification = Some((
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
            self.notification = Some((
                format!("{} already {}", process.label, process.status.label()),
                NotificationType::Warning,
            ));
            return;
        }
        if !process.killable {
            self.notification = Some((
                format!("{} can't be killed from here", process.label),
                NotificationType::Warning,
            ));
            return;
        }
        self.notification = Some((
            format!(
                "Kill {} {}? Press y to confirm",
                process.kind.label(),
                process.label
            ),
            NotificationType::Warning,
        ));
        self.pending_kill = Some(process.id);
    }

    /// Resolve a pending kill with `key`. Returns `false` when nothing was
    /// pending.
    pub(super) fn handle_pending_kill(&mut self, key: KeyEvent) -> bool {
        let Some(id) = self.pending_kill.take() else {
            return false;
        };
        if matches!(key.code, KeyCode::Char('y' | 'Y')) {
            if let Some(tx) = &self.event_tx {
                let _ = tx.try_send(UiEvent::KillProcess(id));
            }
        }
        true
    }

    /// Clicking a tab shows it. Clicking a row highlights it and opens it
    /// over the chat panel at once, the same in every tab: a file, a
    /// process's details, a trace step, a plan (at the clicked todo), a
    /// task's agent. Directories have nothing to open, so a click folds
    /// them; plans and tasks fold when their fold glyph is clicked.
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
        match self.side_tab {
            SidePanelTab::Sandbox => {
                if let Some(index) = self.sandbox_row_at(column, row) {
                    self.sandbox.select(index);
                    self.activate_selected_sandbox_row();
                }
            }
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
            SidePanelTab::ChannelInfo => {}
        }
    }

    fn side_tab_at(&self, column: u16, row: u16) -> Option<SidePanelTab> {
        let channel_open = self.channel_open();
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
        (index < self.processes.len()).then_some(index)
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
        match self.side_tab {
            SidePanelTab::Sandbox => self
                .sandbox_row_at(column, row)
                .map(HoverTarget::SandboxRow),
            SidePanelTab::Processes => self
                .process_row_at(column, row)
                .map(HoverTarget::ProcessRow),
            SidePanelTab::Host => None,
            SidePanelTab::Trace => self.trace_row_at(column, row).map(HoverTarget::TraceRow),
            SidePanelTab::Tasks => self.task_row_at(column, row).map(HoverTarget::TaskRow),
            SidePanelTab::ChannelMembers => self
                .channel_member_row_at(column, row)
                .map(HoverTarget::ChannelMemberRow),
            SidePanelTab::ChannelInfo => None,
        }
    }
}

/// Whether `column` falls on the icon of a side-panel row drawn at
/// `depth` in rows starting at `rows_x`: the icon column of
/// `components::list_row`, one leading space and two per level in.
pub(super) fn on_row_icon(column: u16, rows_x: u16, depth: usize) -> bool {
    let start = 1 + 2 * depth;
    let offset = usize::from(column.saturating_sub(rows_x));
    column >= rows_x && (start..start + crate::animation::AURA_WIDTH + 1).contains(&offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::PanelFocus;
    use crate::events::{ProcessKind, ProcessStatus, UiCommand};
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use tokio::sync::mpsc;

    fn process(id: &str, status: ProcessStatus, killable: bool) -> ProcessSummary {
        ProcessSummary {
            id: id.into(),
            kind: ProcessKind::Command,
            label: format!("job {id}"),
            status,
            owner_agent_id: "owner".into(),
            owner_name: "Agent".into(),
            jump_agent_id: "root".into(),
            killable,
            ..ProcessSummary::default()
        }
    }

    fn app() -> (App, mpsc::Receiver<UiEvent>) {
        let (tx, rx) = mpsc::channel(8);
        let mut app = App::new().with_event_sender(tx);
        app.process_command(UiCommand::SetProcesses(vec![
            process("a", ProcessStatus::Running, true),
            process("b", ProcessStatus::Running, false),
            process("c", ProcessStatus::Exited, true),
        ]));
        app.focus = PanelFocus::Sandbox;
        app.show_side_tab(SidePanelTab::Processes);
        (app, rx)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    #[test]
    fn shift_arrows_switch_tabs_and_plain_arrows_stay_in_the_list() {
        let mut app = App::new();
        app.focus = PanelFocus::Sandbox;
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        assert_eq!(app.side_tab(), SidePanelTab::Processes);
        app.handle_key(key(KeyCode::Left));
        assert_eq!(app.side_tab(), SidePanelTab::Processes);
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
        assert_eq!(app.side_tab(), SidePanelTab::Sandbox);
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
        assert_eq!(app.side_tab(), SidePanelTab::Tasks);
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
        assert_eq!(app.side_tab(), SidePanelTab::Trace);
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
        assert_eq!(app.side_tab(), SidePanelTab::Host);
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
        assert_eq!(app.side_tab(), SidePanelTab::Processes);
        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.side_tab(), SidePanelTab::Processes);
        for _ in 0..4 {
            app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        }
        assert_eq!(app.side_tab(), SidePanelTab::Sandbox);
    }

    #[test]
    fn home_and_end_move_the_list_not_the_input() {
        let (mut app, _rx) = app();
        app.handle_key(key(KeyCode::End));
        assert_eq!(app.selected_process(), 2);
        app.handle_key(key(KeyCode::Home));
        assert_eq!(app.selected_process(), 0);
        assert!(app.input().is_empty());
    }

    #[test]
    fn selection_follows_the_process_across_updates() {
        let (mut app, _rx) = app();
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.selected_process_data().unwrap().id, "b");
        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(app.detail_view(), Some(DetailView::Process(p)) if p.id == "b"));

        let mut finished = process("b", ProcessStatus::Killed, false);
        finished.exit_code = Some(137);
        app.process_command(UiCommand::SetProcesses(vec![
            process("new", ProcessStatus::Running, true),
            process("a", ProcessStatus::Running, true),
            finished,
        ]));
        assert_eq!(app.selected_process(), 2);
        let Some(DetailView::Process(open)) = app.detail_view() else {
            panic!("process view closed");
        };
        assert_eq!(open.status, ProcessStatus::Killed);
        assert_eq!(open.exit_code, Some(137));
    }

    #[test]
    fn kill_asks_first_and_only_for_live_killable_processes() {
        let (mut app, mut rx) = app();
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.pending_kill.as_deref(), Some("a"));
        app.handle_key(key(KeyCode::Char('y')));
        assert!(matches!(rx.try_recv(), Ok(UiEvent::KillProcess(id)) if id == "a"));

        app.handle_key(key(KeyCode::Char('k')));
        app.handle_key(key(KeyCode::Char('n')));
        assert!(rx.try_recv().is_err());
        assert!(app.input().is_empty());

        for index in [1, 2] {
            app.selected_process = index;
            app.handle_key(key(KeyCode::Delete));
            assert!(app.pending_kill.is_none());
            assert!(app.notification().is_some());
        }
    }

    #[test]
    fn o_opens_the_agent_at_the_root_of_the_process() {
        let (mut app, mut rx) = app();
        app.handle_key(key(KeyCode::Char('o')));
        assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "root"));
    }

    #[test]
    fn clicking_tabs_and_rows() {
        let (mut app, _rx) = app();
        app.areas.sandbox = Some(Rect::new(80, 10, 30, 12));
        app.set_side_tabs_area(Rect::new(81, 11, 28, 1));
        app.set_process_rows_area(Rect::new(81, 13, 28, 8), 0);
        let click = |column, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::empty(),
        };

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            ..click(82, 14)
        });
        assert_eq!(app.hover(), Some(HoverTarget::ProcessRow(1)));
        app.handle_mouse(click(82, 14));
        assert_eq!(app.selected_process(), 1);
        assert!(
            matches!(app.detail_view(), Some(DetailView::Process(p)) if p.id == "b"),
            "one click opens the process"
        );

        app.close_detail();
        app.handle_mouse(click(82, 11));
        assert_eq!(app.side_tab(), SidePanelTab::Sandbox);
    }

    #[test]
    fn trace_rows_hover_and_open_on_one_click() {
        use crate::events::{TraceNode, TraceNodeStatus};
        let mut app = App::new();
        app.trace_feed.push(
            "",
            "r",
            &[TraceNode {
                id: "s1".into(),
                kind: "iteration".into(),
                label: "step".into(),
                status: TraceNodeStatus::Ok,
                ..TraceNode::default()
            }],
        );
        app.close_detail();
        app.show_side_tab(SidePanelTab::Trace);
        app.areas.sandbox = Some(Rect::new(80, 10, 30, 12));
        app.set_trace_rows_area(Rect::new(81, 13, 28, 8), 0);
        assert_eq!(
            app.hovered_side_panel(82, 13),
            Some(HoverTarget::TraceRow(0))
        );
        app.click_side_panel(82, 13);
        assert!(matches!(app.detail_view(), Some(DetailView::File(_))));
    }
}
