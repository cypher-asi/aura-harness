//! The right-hand panel's tabs, and its Processes list: everything agents
//! and the user have running, with the agent that started each one.

use super::{contains, follow_selection, App, DetailView, HoverTarget, NotificationType};
use crate::app::file_view::FileView;
use crate::app::sandbox::MAX_COUNTED_FILES;
use crate::events::{ProcessSummary, UiEvent};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

/// Views of the right-hand panel below the Record panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidePanelTab {
    /// File tree of the active agent's workspace.
    #[default]
    Sandbox,
    /// Live agent turns, spawned agents and background commands.
    Processes,
}

impl SidePanelTab {
    pub const ALL: [Self; 2] = [Self::Sandbox, Self::Processes];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sandbox => "SANDBOX",
            Self::Processes => "PROCESSES",
        }
    }

    const fn other(self) -> Self {
        match self {
            Self::Sandbox => Self::Processes,
            Self::Processes => Self::Sandbox,
        }
    }
}

impl App {
    /// Which view the panel below the Record panel shows.
    #[must_use]
    pub const fn side_tab(&self) -> SidePanelTab {
        self.side_tab
    }

    /// The count shown beside `tab`'s label: files in the sandbox, or
    /// processes still running. `None` before there is a sandbox.
    #[must_use]
    pub fn side_tab_count(&self, tab: SidePanelTab) -> Option<String> {
        match tab {
            SidePanelTab::Sandbox => self.sandbox.file_count().map(|files| {
                if files >= MAX_COUNTED_FILES {
                    format!("{files}+")
                } else {
                    files.to_string()
                }
            }),
            SidePanelTab::Processes => Some(
                self.processes
                    .iter()
                    .filter(|process| process.status.is_live())
                    .count()
                    .to_string(),
            ),
        }
    }

    /// Each tab with the first column and width of its ` label count `
    /// cell in the tab bar, as drawn and as clicks are mapped back.
    pub fn side_tab_layout(&self) -> impl Iterator<Item = (SidePanelTab, u16, u16)> + '_ {
        super::tab_layout(SidePanelTab::ALL, |tab| {
            match self.side_tab_count(tab) {
                Some(count) => format!("{} {count}", tab.label()),
                None => tab.label().to_string(),
            }
        })
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

    /// Record where the side panel's tab bar was drawn this frame.
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

    pub(super) fn show_side_tab(&mut self, tab: SidePanelTab) {
        self.side_tab = tab;
        self.pending_kill = None;
    }

    pub(super) fn toggle_side_tab(&mut self) {
        self.show_side_tab(self.side_tab.other());
    }

    /// Replace the list, keeping the highlight on the same process when it
    /// is still listed, and refreshing an open process detail view.
    pub(super) fn set_processes(&mut self, processes: Vec<ProcessSummary>) {
        let selected_id = self
            .processes
            .get(self.selected_process)
            .map(|p| p.id.clone());
        self.processes = processes;
        self.selected_process = selected_id
            .and_then(|id| self.processes.iter().position(|p| p.id == id))
            .unwrap_or(self.selected_process)
            .min(self.processes.len().saturating_sub(1));
        if let Some(DetailView::Process(open)) = self.detail.as_mut() {
            if let Some(fresh) = self.processes.iter().find(|p| p.id == open.id) {
                **open = fresh.clone();
            }
        }
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
            KeyCode::PageUp => self.move_process_selection(-5),
            KeyCode::PageDown => self.move_process_selection(5),
            KeyCode::Home => self.select_process(0),
            KeyCode::End => self.select_process(usize::MAX),
            KeyCode::Enter => self.open_selected_process(),
            KeyCode::Left | KeyCode::Right => self.toggle_side_tab(),
            _ => {}
        }
    }

    /// Shift+Left/Right switches the side panel's tab from either tab.
    pub(super) fn handle_side_tab_key(&mut self, key: KeyEvent) -> bool {
        let switch = key.modifiers.contains(KeyModifiers::SHIFT)
            && matches!(key.code, KeyCode::Left | KeyCode::Right);
        if switch {
            self.toggle_side_tab();
        }
        switch
    }

    /// Letter shortcuts of the Processes tab, used while the input is empty.
    pub(super) fn handle_processes_shortcut(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('o') => self.jump_to_selected_owner(),
            KeyCode::Char('k') | KeyCode::Delete => self.request_kill_selected(),
            KeyCode::Char('l') => self.open_selected_log(),
            _ => return false,
        }
        true
    }

    fn open_selected_process(&mut self) {
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

    /// Clicking a tab shows it; clicking a row highlights it, and clicking
    /// the highlighted row opens it.
    pub(super) fn click_side_panel(&mut self, column: u16, row: u16) {
        if let Some(tab) = self.side_tab_at(column, row) {
            self.show_side_tab(tab);
            return;
        }
        match self.side_tab {
            SidePanelTab::Sandbox => self.click_sandbox_row(column, row),
            SidePanelTab::Processes => {
                if let Some(index) = self.process_row_at(column, row) {
                    if index == self.selected_process {
                        self.open_selected_process();
                    }
                    self.selected_process = index;
                }
            }
        }
    }

    fn side_tab_at(&self, column: u16, row: u16) -> Option<SidePanelTab> {
        let bar = self
            .areas
            .side_tabs
            .filter(|&bar| contains(Some(bar), column, row))?;
        let offset = column - bar.x;
        self.side_tab_layout()
            .find(|&(_, start, width)| offset >= start && offset < start.saturating_add(width))
            .map(|(tab, _, _)| tab)
    }

    fn process_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let rows = self
            .areas
            .process_rows
            .filter(|&rows| contains(Some(rows), column, row))?;
        let index = self.areas.processes_scroll + usize::from(row - rows.y);
        (index < self.processes.len()).then_some(index)
    }

    /// Clickable item of the side panel at a screen cell.
    pub(super) fn hovered_side_panel(&self, column: u16, row: u16) -> Option<HoverTarget> {
        if let Some(tab) = self.side_tab_at(column, row) {
            return Some(HoverTarget::SideTab(tab));
        }
        if self.side_tab == SidePanelTab::Processes {
            return self
                .process_row_at(column, row)
                .map(HoverTarget::ProcessRow);
        }
        None
    }
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
    fn shift_arrows_switch_tabs_from_either_tab() {
        let mut app = App::new();
        app.focus = PanelFocus::Sandbox;
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        assert_eq!(app.side_tab(), SidePanelTab::Processes);
        app.handle_key(key(KeyCode::Left));
        assert_eq!(app.side_tab(), SidePanelTab::Sandbox);
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
        assert_eq!(app.side_tab(), SidePanelTab::Processes);
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
        assert!(app.detail_view().is_none());
        app.handle_mouse(click(82, 14));
        assert!(matches!(app.detail_view(), Some(DetailView::Process(_))));

        app.close_detail();
        app.handle_mouse(click(82, 11));
        assert_eq!(app.side_tab(), SidePanelTab::Sandbox);
    }
}
