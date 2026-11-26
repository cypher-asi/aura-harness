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

use std::collections::{HashMap, HashSet};

use super::{
    contains, follow_selection, processes::SIDE_PAGE, tab_at, tab_bar::TabLayout, App, DetailView,
    NotificationType, PanelFocus,
};
use crate::events::{ProcessKind, ProcessStatus, ProcessSummary};
use aura_core_types::time::now_ms;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

/// Members kept, across every agent's runs; the oldest go first.
const MAX_SEEN: usize = 1000;

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

/// State of the MEMBERS tab.
#[derive(Debug, Default)]
pub(crate) struct Members {
    /// The latest row of every `task` spawn and background agent seen,
    /// oldest first
    seen: Vec<ProcessSummary>,
    /// When each agent's run began, in Unix milliseconds, for agents
    /// that started another with `/new`
    since: HashMap<String, u64>,
    /// Rows listed for the agent on screen
    rows: Vec<MemberRow>,
    /// Highlighted row
    selected: usize,
    /// First row on screen
    scroll: usize,
}

/// Hex id of the agent a process row of a `task` spawn or a background
/// agent stands for: what follows `task:` or `agent:` in its id.
fn member_id(process: &ProcessSummary) -> Option<&str> {
    if !matches!(process.kind, ProcessKind::Task | ProcessKind::Agent) {
        return None;
    }
    process
        .id
        .split_once(':')
        .map(|(_, id)| id)
        .filter(|id| !id.is_empty())
}

impl Members {
    /// Keep the latest row of every member in `processes`.
    fn absorb(&mut self, processes: &[ProcessSummary]) {
        for process in processes {
            let Some(id) = member_id(process) else {
                continue;
            };
            match self
                .seen
                .iter_mut()
                .find(|seen| member_id(seen) == Some(id))
            {
                Some(seen) => *seen = process.clone(),
                None => self.seen.push(process.clone()),
            }
        }
        let over = self.seen.len().saturating_sub(MAX_SEEN);
        self.seen.drain(..over);
    }

    /// List `lead` and the members of its run, keeping the highlight on
    /// the same agent when it is still listed.
    fn rebuild(&mut self, lead: Option<MemberRow>) {
        let kept = self.rows.get(self.selected).map(|row| row.agent_id.clone());
        self.rows = lead.map_or_else(Vec::new, |lead| self.run_of(lead));
        self.selected = kept
            .and_then(|id| self.rows.iter().position(|row| row.agent_id == id))
            .unwrap_or(self.selected)
            .min(self.rows.len().saturating_sub(1));
    }

    /// `lead`, then every agent its run brought in: the ones it started
    /// since its run began, and the ones they started, each under the
    /// member that started it, oldest first.
    fn run_of(&self, lead: MemberRow) -> Vec<MemberRow> {
        let lead_id = lead.agent_id.clone();
        let since = self.since.get(&lead_id).copied().unwrap_or(0);
        let candidates: Vec<(&str, &ProcessSummary)> = self
            .seen
            .iter()
            .filter(|process| process.started_ms >= since)
            .filter_map(|process| Some((member_id(process)?, process)))
            .filter(|(id, _)| *id != lead_id)
            .collect();

        // An agent joins when the one that started it, or one above
        // that, is in the run. Agents a member started can come before
        // it in the table, so look again until no one else joins.
        let mut joined: HashSet<&str> = HashSet::from([lead_id.as_str()]);
        let mut members: Vec<usize> = Vec::new();
        loop {
            let before = members.len();
            for (i, &(id, process)) in candidates.iter().enumerate() {
                if joined.contains(id) {
                    continue;
                }
                let mut starters =
                    std::iter::once(&process.owner_agent_id).chain(&process.parent_chain);
                if starters.any(|id| joined.contains(id.as_str())) {
                    joined.insert(id);
                    members.push(i);
                }
            }
            if members.len() == before {
                break;
            }
        }

        // Each member goes under the nearest agent above it in the run.
        let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
        for &i in &members {
            let process = candidates[i].1;
            let parent = std::iter::once(&process.owner_agent_id)
                .chain(&process.parent_chain)
                .map(String::as_str)
                .find(|id| joined.contains(id) && *id != candidates[i].0)
                .unwrap_or(lead_id.as_str());
            children.entry(parent).or_default().push(i);
        }
        for list in children.values_mut() {
            list.sort_by_key(|&i| (candidates[i].1.started_ms, i));
        }

        let mut rows = Vec::with_capacity(members.len() + 1);
        rows.push(lead);
        let mut placed: HashSet<usize> = HashSet::new();
        let mut stack: Vec<(usize, usize)> = children
            .get(lead_id.as_str())
            .map(|list| list.iter().rev().map(|&i| (i, 1)).collect())
            .unwrap_or_default();
        loop {
            while let Some((i, depth)) = stack.pop() {
                if !placed.insert(i) {
                    continue;
                }
                let (id, process) = candidates[i];
                rows.push(member_row(id, process, depth));
                if let Some(list) = children.get(id) {
                    stack.extend(list.iter().rev().map(|&child| (child, depth + 1)));
                }
            }
            // Members whose starters only start each other hang under the
            // lead.
            match members.iter().find(|&&i| !placed.contains(&i)) {
                Some(&i) => stack.push((i, 1)),
                None => break,
            }
        }
        rows
    }

    /// Rows listed for the agent on screen.
    pub(super) fn rows(&self) -> &[MemberRow] {
        &self.rows
    }

    fn select(&mut self, index: usize) {
        self.selected = index.min(self.rows.len().saturating_sub(1));
    }

    fn move_by(&mut self, delta: isize) {
        self.select(self.selected.saturating_add_signed(delta));
    }
}

fn member_row(id: &str, process: &ProcessSummary, depth: usize) -> MemberRow {
    MemberRow {
        agent_id: id.to_string(),
        label: process.label.clone(),
        depth,
        status: process.status,
        kind: Some(process.kind),
        started_ms: process.started_ms,
        ended_ms: process.ended_ms,
        process: Some(process.clone()),
    }
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
        self.record_panel_visible = true;
        self.side_expanded = false;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{AgentSummary, UiCommand, UiEvent};
    use tokio::sync::mpsc;

    const LEAD: &str = "aa";

    fn member(id: &str, kind: ProcessKind, owner: &str, started_ms: u64) -> ProcessSummary {
        let prefix = if kind == ProcessKind::Task {
            "task"
        } else {
            "agent"
        };
        ProcessSummary {
            id: format!("{prefix}:{id}"),
            kind,
            label: format!("{id} (scout)"),
            status: ProcessStatus::Running,
            owner_agent_id: owner.into(),
            started_ms,
            jump_agent_id: if kind == ProcessKind::Task {
                id.into()
            } else {
                LEAD.into()
            },
            ..ProcessSummary::default()
        }
    }

    fn app_with_lead() -> App {
        let mut app = App::new();
        app.process_command(UiCommand::SetAgents(vec![AgentSummary {
            id: LEAD.into(),
            name: "Lead".into(),
            ..AgentSummary::default()
        }]));
        app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
        app
    }

    fn listed(app: &App) -> Vec<(String, usize)> {
        app.members()
            .iter()
            .map(|row| (row.agent_id.clone(), row.depth))
            .collect()
    }

    fn rows(pairs: &[(&str, usize)]) -> Vec<(String, usize)> {
        pairs.iter().map(|&(id, d)| (id.to_string(), d)).collect()
    }

    #[test]
    fn the_lead_comes_first_and_members_sit_under_who_started_them() {
        let mut app = app_with_lead();
        let mut grandchild = member("cc", ProcessKind::Agent, "bb", 30);
        grandchild.parent_chain = vec![LEAD.into()];
        app.process_command(UiCommand::SetProcesses(vec![
            grandchild,
            member("bb", ProcessKind::Task, LEAD, 10),
            member("dd", ProcessKind::Task, LEAD, 20),
        ]));
        assert_eq!(
            listed(&app),
            rows(&[(LEAD, 0), ("bb", 1), ("cc", 2), ("dd", 1)])
        );
        assert_eq!(app.members()[0].label, "Lead");
        assert_eq!(app.members()[0].kind, None);
    }

    #[test]
    fn agents_a_task_member_spawned_are_members_too() {
        let mut app = app_with_lead();
        // A task's instance roots its own tree of background agents, so
        // their rows name it alone above them.
        app.process_command(UiCommand::SetProcesses(vec![
            member("ee", ProcessKind::Agent, "bb", 40),
            member("bb", ProcessKind::Task, LEAD, 10),
        ]));
        assert_eq!(listed(&app), rows(&[(LEAD, 0), ("bb", 1), ("ee", 2)]));
    }

    #[test]
    fn other_runs_and_other_work_are_left_out() {
        let mut app = app_with_lead();
        let command = ProcessSummary {
            id: "cmd:1".into(),
            kind: ProcessKind::Command,
            owner_agent_id: LEAD.into(),
            ..ProcessSummary::default()
        };
        let turn = ProcessSummary {
            id: format!("turn:{LEAD}"),
            kind: ProcessKind::Turn,
            owner_agent_id: LEAD.into(),
            owner_name: "Lead".into(),
            ..ProcessSummary::default()
        };
        app.process_command(UiCommand::SetProcesses(vec![
            turn,
            command,
            member("bb", ProcessKind::Task, LEAD, 10),
            member("zz", ProcessKind::Task, "ff", 10),
        ]));
        assert_eq!(listed(&app), rows(&[(LEAD, 0), ("bb", 1)]));
        assert_eq!(app.members()[0].status, ProcessStatus::Running);

        // Another agent on screen lists its own run.
        app.process_command(UiCommand::SetActiveAgent("ff".into()));
        assert_eq!(listed(&app), rows(&[("ff", 0), ("zz", 1)]));
    }

    #[test]
    fn members_stay_listed_after_the_process_table_drops_them() {
        let mut app = app_with_lead();
        app.process_command(UiCommand::SetProcesses(vec![member(
            "bb",
            ProcessKind::Task,
            LEAD,
            10,
        )]));
        let mut done = member("bb", ProcessKind::Task, LEAD, 10);
        done.status = ProcessStatus::Exited;
        done.ended_ms = Some(50);
        app.process_command(UiCommand::SetProcesses(vec![done]));
        app.process_command(UiCommand::SetProcesses(Vec::new()));
        assert_eq!(listed(&app), rows(&[(LEAD, 0), ("bb", 1)]));
        assert_eq!(app.members()[1].status, ProcessStatus::Exited);
        assert_eq!(app.members()[1].ended_ms, Some(50));
    }

    #[test]
    fn a_new_session_starts_a_run_no_one_has_joined() {
        let mut app = app_with_lead();
        app.process_command(UiCommand::SetProcesses(vec![member(
            "bb",
            ProcessKind::Task,
            LEAD,
            10,
        )]));
        app.handle_command("/new");
        assert_eq!(listed(&app), rows(&[(LEAD, 0)]));

        let later = now_ms() + 1_000;
        app.process_command(UiCommand::SetProcesses(vec![member(
            "cc",
            ProcessKind::Task,
            LEAD,
            later,
        )]));
        assert_eq!(listed(&app), rows(&[(LEAD, 0), ("cc", 1)]));
    }

    #[test]
    fn the_highlight_stays_on_its_member_as_others_join() {
        let mut app = app_with_lead();
        app.process_command(UiCommand::SetProcesses(vec![
            member("bb", ProcessKind::Task, LEAD, 10),
            member("dd", ProcessKind::Task, LEAD, 20),
        ]));
        app.show_members();
        assert!(app.handle_records_shortcut(KeyEvent::from(KeyCode::End)));
        assert_eq!(app.members()[app.selected_member()].agent_id, "dd");
        app.process_command(UiCommand::SetProcesses(vec![
            member("bb", ProcessKind::Task, LEAD, 10),
            member("cc", ProcessKind::Task, LEAD, 15),
            member("dd", ProcessKind::Task, LEAD, 20),
        ]));
        assert_eq!(app.members()[app.selected_member()].agent_id, "dd");
    }

    #[test]
    fn enter_opens_a_members_process_and_o_its_chat() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut app = app_with_lead().with_event_sender(tx);
        app.process_command(UiCommand::SetProcesses(vec![
            member("bb", ProcessKind::Task, LEAD, 10),
            member("cc", ProcessKind::Agent, LEAD, 20),
        ]));
        app.show_members();
        app.handle_members_key(KeyEvent::from(KeyCode::Down));
        app.handle_members_key(KeyEvent::from(KeyCode::Enter));
        assert!(
            matches!(app.detail_view(), Some(DetailView::Process(p)) if p.id == "task:bb"),
            "Enter opens the member's process"
        );

        assert!(app.handle_records_shortcut(KeyEvent::from(KeyCode::Char('o'))));
        assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));

        // A background agent's chat is its tree's, so there is none to open.
        app.handle_members_key(KeyEvent::from(KeyCode::Down));
        assert!(app.handle_records_shortcut(KeyEvent::from(KeyCode::Char('o'))));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn shift_left_and_right_switch_the_kernel_panels_tab() {
        let mut app = app_with_lead();
        assert_eq!(app.records_tab(), RecordsTab::Kernel);
        let shift_right = KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT);
        assert!(app.handle_records_shortcut(shift_right));
        assert_eq!(app.records_tab(), RecordsTab::Members);
        assert!(app.handle_records_shortcut(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT)));
        assert_eq!(app.records_tab(), RecordsTab::Kernel);
        // Plain letters stay the input's while KERNEL is open.
        assert!(!app.handle_records_shortcut(KeyEvent::from(KeyCode::Char('o'))));
    }

    #[test]
    fn keys_reach_the_members_tab_through_the_focused_kernel_panel() {
        let mut app = app_with_lead();
        app.process_command(UiCommand::SetProcesses(vec![member(
            "bb",
            ProcessKind::Task,
            LEAD,
            10,
        )]));
        app.focus = PanelFocus::Records;
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        assert_eq!(app.records_tab(), RecordsTab::Members);
        app.handle_key(KeyEvent::from(KeyCode::Down));
        assert_eq!(app.selected_member(), 1);
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert!(matches!(app.detail_view(), Some(DetailView::Process(p)) if p.id == "task:bb"));

        // A record jump goes back to the records.
        app.close_detail();
        app.show_turn_records(0);
        assert_eq!(app.records_tab(), RecordsTab::Members, "no records to show");
        app.process_command(UiCommand::SetRecords(vec![
            crate::app::detail_tests::record(1),
        ]));
        app.show_turn_records(0);
        assert_eq!(app.records_tab(), RecordsTab::Kernel);
        assert_eq!(app.focus(), PanelFocus::Records);
    }

    #[test]
    fn clicking_a_tab_in_the_kernel_bar_shows_it() {
        let mut app = app_with_lead();
        let bar = Rect::new(80, 0, 40, 3);
        app.set_record_tabs_area(bar);
        let members = RecordsTab::layout(bar.width).cells[1];
        app.click_records_panel(bar.x + members.start, 1);
        assert_eq!(app.records_tab(), RecordsTab::Members);
    }

    #[test]
    fn the_members_command_shows_the_tab_focused() {
        let mut app = app_with_lead();
        app.handle_command("/kernel");
        assert!(!app.record_panel_visible());
        app.handle_command("/members");
        assert!(app.record_panel_visible());
        assert_eq!(app.records_tab(), RecordsTab::Members);
        assert_eq!(app.focus(), PanelFocus::Records);
    }
}
