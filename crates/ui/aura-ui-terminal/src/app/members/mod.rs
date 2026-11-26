//! The Kernel panel's tabs: KERNEL lists the kernel's records of the
//! agent on screen, MEMBERS the agents that agent's run brought in,
//! POLICIES the policy its kernel enforces (see [`super::policies`]), and
//! INBOX the messages the run's agents sent each other (see
//! [`super::inbox`]).
//!
//! Members come from the process table ([`UiCommand::SetProcesses`]). Every
//! `task` spawn and background agent it lists is kept here, so an agent
//! stays a member after the table drops its row to make room. The agent on
//! screen leads the list; below it, each agent it started, with the
//! agents that one started under it, and so on down. An agent's run is its
//! conversation: `/new` starts another, which no agent has joined yet.
//!
//! [`UiCommand::SetProcesses`]: crate::events::UiCommand::SetProcesses

use super::{
    contains, follow_selection, processes::SIDE_PAGE, tab_at, tab_bar::TabLayout, App, DetailView,
    NotificationType, PanelFocus,
};
use crate::events::{ProcessKind, ProcessStatus, ProcessSummary};
use aura_core_types::time::now_ms;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
pub(crate) use run::Members;

/// Views of the Kernel panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecordsTab {
    /// The kernel's records of the agent on screen.
    #[default]
    Kernel,
    /// The agents the run of the agent on screen brought in.
    Members,
    /// The policy the kernel of the agent on screen enforces.
    Policies,
    /// The messages the agents of that run sent each other, by thread.
    Inbox,
}

impl RecordsTab {
    pub const ALL: [Self; 4] = [Self::Kernel, Self::Members, Self::Policies, Self::Inbox];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Kernel => "KERNEL",
            Self::Members => "MEMBERS",
            Self::Policies => "POLICIES",
            Self::Inbox => "INBOX",
        }
    }

    /// Label for header bars too narrow for `label`.
    #[must_use]
    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Kernel => "KRN",
            Self::Members => "MEM",
            Self::Policies => "POL",
            Self::Inbox => "INB",
        }
    }

    /// The tab after this one, wrapping around, as Shift+Right goes.
    const fn next(self) -> Self {
        match self {
            Self::Kernel => Self::Members,
            Self::Members => Self::Policies,
            Self::Policies => Self::Inbox,
            Self::Inbox => Self::Kernel,
        }
    }

    /// The tab before this one, wrapping around, as Shift+Left goes.
    const fn prev(self) -> Self {
        match self {
            Self::Kernel => Self::Inbox,
            Self::Members => Self::Kernel,
            Self::Policies => Self::Members,
            Self::Inbox => Self::Policies,
        }
    }

    /// The tabs' cells in a header bar `width` columns wide, with the open
    /// tab's count after them. The renderer draws the bar with this layout
    /// and clicks are mapped back through it.
    #[must_use]
    pub fn layout(width: u16) -> TabLayout<Self> {
        TabLayout::with_info(Self::ALL, Self::label, Self::short_label, width)
    }
}

/// One row of the MEMBERS tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberRow {
    /// Hex id of the agent
    pub agent_id: String,
    /// Its name, and the permanent agent or role it is an instance of
    pub label: String,
    /// Levels below the agent on screen: 0 for it, 1 for the agents it
    /// started, and so on
    pub depth: usize,
    pub status: ProcessStatus,
    /// How it joined: a `task`, or a background agent. `None` for the
    /// agent on screen, which leads the run.
    pub kind: Option<ProcessKind>,
    /// When it joined, in Unix milliseconds
    pub started_ms: u64,
    /// When it finished, in Unix milliseconds
    pub ended_ms: Option<u64>,
    /// Its row in the process table as last seen, which Enter opens
    pub process: Option<ProcessSummary>,
}

impl App {
    /// Which list the Kernel panel shows.
    #[must_use]
    pub const fn records_tab(&self) -> RecordsTab {
        self.records_tab
    }

    pub(super) fn show_records_tab(&mut self, tab: RecordsTab) {
        let opened = tab != self.records_tab;
        self.records_tab = tab;
        if opened && tab == RecordsTab::Policies {
            self.request_policies();
        }
    }

    /// Show the Kernel panel's MEMBERS tab, focused, as `/members` does.
    /// In CHANNELS, and in BENCH but on an agent of a benchmark, this only
    /// says so (see [`Self::show_inbox`]).
    pub(super) fn show_members(&mut self) {
        if self.agent_panels_hidden() {
            self.notify_no_agent_panels();
            return;
        }
        self.panels.record_visible = true;
        self.panels.side_expanded = false;
        self.show_records_tab(RecordsTab::Members);
        self.focus = PanelFocus::Records;
    }

    /// Record where the Kernel panel's header bar was drawn this frame.
    pub fn set_record_tabs_area(&mut self, area: Rect) {
        self.areas.record_tabs = Some(area);
    }

    /// The agents of the run of the agent on screen: it first, then the
    /// agents its run brought in, each under the one that started it.
    #[must_use]
    pub fn members(&self) -> &[MemberRow] {
        &self.members.rows
    }

    /// Highlighted row of the MEMBERS tab.
    #[must_use]
    pub const fn selected_member(&self) -> usize {
        self.members.selected
    }

    /// Keep the highlighted member inside a list `visible_rows` tall,
    /// returning the first row to show.
    pub fn sync_members_scroll(&mut self, visible_rows: usize) -> usize {
        self.members.scroll = follow_selection(
            self.members.scroll,
            self.members.selected,
            visible_rows,
            self.members.rows.len(),
        );
        self.members.scroll
    }

    /// Record where the member rows were drawn this frame and which row is
    /// at the top.
    pub fn set_member_rows_area(&mut self, area: Rect, scroll: usize) {
        self.areas.member_rows = Some(area);
        self.areas.members_scroll = scroll;
    }

    /// Take in the process table just sent, then list the members again.
    pub(super) fn absorb_members(&mut self) {
        self.members.absorb(&self.all_processes);
        self.refresh_members();
    }

    /// List the members of the run of the agent on screen again, after
    /// the processes, the agents or the agent on screen changed.
    pub(super) fn refresh_members(&mut self) {
        let lead = self.lead_member();
        self.members.rebuild(lead);
        self.refresh_inbox();
    }

    /// Start a new run of the agent on screen, as `/new` does: the agents
    /// its earlier runs brought in are no longer listed.
    pub(super) fn start_members_run(&mut self) {
        if self.active_agent_id.is_empty() {
            return;
        }
        self.members
            .since
            .insert(self.active_agent_id.clone(), now_ms());
        self.refresh_members();
    }

    /// The row of the agent on screen, which leads its run.
    fn lead_member(&self) -> Option<MemberRow> {
        if self.active_agent_id.is_empty() {
            return None;
        }
        let id = &self.active_agent_id;
        let agent = self.agents.iter().find(|agent| agent.id == *id);
        let turn = self.all_processes.iter().find(|process| {
            process.kind == ProcessKind::Turn
                && process.owner_agent_id == *id
                && process.status.is_live()
        });
        let name = agent
            .map(|agent| agent.name.clone())
            .filter(|name| !name.is_empty())
            .or_else(|| turn.map(|turn| turn.owner_name.clone()))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "This agent".to_string());
        let label = match agent.and_then(|agent| agent.definition.as_deref()) {
            Some(definition) if definition != name => format!("{name} ({definition})"),
            _ => name,
        };
        let running = turn.is_some() || agent.is_some_and(|agent| agent.running);
        Some(MemberRow {
            agent_id: id.clone(),
            label,
            depth: 0,
            status: if running {
                ProcessStatus::Running
            } else {
                ProcessStatus::Idle
            },
            kind: None,
            started_ms: turn.map_or(0, |turn| turn.started_ms),
            ended_ms: None,
            process: turn.cloned(),
        })
    }

    pub(super) fn member_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let rows = self
            .areas
            .member_rows
            .filter(|&rows| contains(Some(rows), column, row))?;
        let index = self.areas.members_scroll + usize::from(row - rows.y);
        (index < self.members.rows.len()).then_some(index)
    }

    /// Clicking a tab shows it; clicking a row acts on the open list.
    pub(super) fn click_records_panel(&mut self, column: u16, row: u16) {
        if contains(self.areas.record_tabs, column, row) {
            if let Some(tab) = tab_at(self.areas.record_tabs, column, row, RecordsTab::layout) {
                self.show_records_tab(tab);
            }
            return;
        }
        match self.records_tab {
            RecordsTab::Kernel => self.click_record_row(column, row),
            RecordsTab::Members => {
                if let Some(index) = self.member_row_at(column, row) {
                    self.members.select(index);
                    self.open_selected_member();
                }
            }
            // A read-only document: clicks only focus the panel.
            RecordsTab::Policies => {}
            RecordsTab::Inbox => {
                if let Some(index) = self.inbox_row_at(column, row) {
                    self.click_inbox_row(index);
                }
            }
        }
    }

    /// Move the MEMBERS tab's highlight by `delta` rows, as the wheel does.
    pub(super) fn scroll_members(&mut self, delta: isize) {
        self.members.move_by(delta);
    }

    /// Keys of the Kernel panel that would otherwise go to the input, used
    /// while it is empty: Shift+Left/Right switch its tab, on the MEMBERS
    /// tab Home/End jump and `o` opens the member's chat, and on the
    /// POLICIES tab Home/End jump.
    pub(super) fn handle_records_shortcut(&mut self, key: KeyEvent) -> bool {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return false;
        }
        if key.modifiers.contains(KeyModifiers::SHIFT)
            && matches!(key.code, KeyCode::Left | KeyCode::Right)
        {
            let tab = if key.code == KeyCode::Right {
                self.records_tab.next()
            } else {
                self.records_tab.prev()
            };
            self.show_records_tab(tab);
            return true;
        }
        if self.records_tab == RecordsTab::Policies {
            return self.handle_policies_shortcut(key);
        }
        if self.records_tab == RecordsTab::Inbox {
            return self.handle_inbox_shortcut(key);
        }
        if self.records_tab != RecordsTab::Members {
            return false;
        }
        match key.code {
            KeyCode::Home => self.members.select(0),
            KeyCode::End => self.members.select(usize::MAX),
            KeyCode::Char('o') => self.open_selected_member_chat(),
            _ => return false,
        }
        true
    }

    /// Navigation keys of the MEMBERS tab.
    pub(super) fn handle_members_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.members.move_by(-1),
            KeyCode::Down => self.members.move_by(1),
            KeyCode::PageUp => self.members.move_by(-SIDE_PAGE),
            KeyCode::PageDown => self.members.move_by(SIDE_PAGE),
            KeyCode::Enter => self.open_selected_member(),
            _ => {}
        }
    }

    /// Show the highlighted member's process over the chat: what it was
    /// asked, its model, when it started and how it ended.
    fn open_selected_member(&mut self) {
        let Some(row) = self.members.rows.get(self.members.selected) else {
            return;
        };
        match row.process.clone() {
            Some(process) => self.open_detail(DetailView::Process(Box::new(process))),
            None => {
                self.notifications.current = Some((
                    format!("{} leads this run and is not working now", row.label),
                    NotificationType::Success,
                ));
            }
        }
    }

    /// Show the highlighted member's chat, for a member that has one.
    fn open_selected_member_chat(&mut self) {
        let Some(row) = self.members.rows.get(self.members.selected) else {
            return;
        };
        let target = row.process.as_ref().map(|process| {
            if process.jump_agent_id.is_empty() {
                process.owner_agent_id.clone()
            } else {
                process.jump_agent_id.clone()
            }
        });
        match target {
            _ if row.kind.is_none() => {
                self.notifications.current = Some((
                    format!("{} is already open", row.label),
                    NotificationType::Success,
                ));
            }
            // A background agent's row opens the agent at the top of its
            // tree, which is this run's lead or another member.
            Some(target) if target == row.agent_id => self.request_agent_switch(target),
            _ => {
                self.notifications.current = Some((
                    format!("{} has no chat of its own", row.label),
                    NotificationType::Warning,
                ));
            }
        }
    }
}

mod run;
#[cfg(test)]
mod tests;
