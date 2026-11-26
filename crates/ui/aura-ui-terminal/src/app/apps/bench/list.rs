//! BENCH's list: every benchmark, newest first, then "+ New bench",
//! which the panel pins to its bottom row. A benchmark unfolds over the
//! agents that work for it, the ones listed under its own agent (whose id
//! it shares) in the projects list or, once archived, the Archive: an
//! agent per arm, which only labels the runs under it, then the runs and
//! the agents they spawned.
//!
//! - Benchmarks start folded. Space on one, or a click on the arrow
//!   before its status, unfolds or folds it; Space on one of its agents
//!   folds it, and the highlight moves onto it. A benchmark unfolds when
//!   one of its agents is shown, and when the form starts it.
//! - The highlight stays on its row as rows come and go, and passes over
//!   the arms.
//! - Enter or a click on a benchmark shows it, Enter handing it the keys.
//!   On an agent it shows the agent's chat, which takes the keys so that
//!   what is typed goes to the agent rather than to the list's own `n`,
//!   `r` and `c`; Esc on CHAT hands them back.

use super::{BenchMode, BenchRequest};
use crate::app::{contains, follow_selection, App, PanelFocus};
use crate::events::{starts_project, AgentSummary};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use std::ops::Range;

/// Rows a PageUp or PageDown moves the highlight by.
const PAGE_STEP: isize = 5;

/// Columns from a row's left edge where a click folds a benchmark: its
/// arrow and the blanks either side of it.
const FOLD_COLUMNS: u16 = 3;

/// A row of BENCH's list as drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchRow {
    /// Benchmark `index` of [`super::BenchState::benches`]
    Bench(usize),
    /// An agent of an unfolded benchmark
    Agent(BenchAgent),
    /// "+ New bench", pinned under the others
    New,
}

/// An agent that works for a benchmark, where the list finds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BenchAgent {
    /// Index of the benchmark in [`super::BenchState::benches`]
    pub bench: usize,
    /// Whether it is in [`App::archived`] rather than [`App::agents`]
    pub archived: bool,
    /// Its index there
    pub index: usize,
    /// Depth there of the benchmark's own agent, which the tree hangs from
    pub top: usize,
    /// Whether it is an arm's, which labels the runs under it and takes
    /// no highlight
    pub arm: bool,
}

/// Where a benchmark's agents are listed: in the Archive or not, under
/// its own agent at depth `top`, at indices `agents`.
struct BenchTree {
    archived: bool,
    top: usize,
    agents: Range<usize>,
}

/// Whether `row` takes the highlight: all but an arm's.
const fn takes_highlight(row: BenchRow) -> bool {
    !matches!(row, BenchRow::Agent(BenchAgent { arm: true, .. }))
}

/// The row nearest `target` that takes the highlight, looking on past it
/// the way the highlight moves first, `down` or up.
fn landing(rows: &[BenchRow], target: usize, down: bool) -> usize {
    let takes = |&row: &usize| takes_highlight(rows[row]);
    let (ahead, behind) = if down {
        (
            (target..rows.len()).find(takes),
            (0..target).rev().find(takes),
        )
    } else {
        (
            (0..=target).rev().find(takes),
            (target + 1..rows.len()).find(takes),
        )
    };
    ahead.or(behind).unwrap_or(target)
}

impl App {
    /// BENCH's list as drawn, "+ New bench" last.
    #[must_use]
    pub fn bench_rows(&self) -> Vec<BenchRow> {
        let mut rows = Vec::with_capacity(self.bench.benches.len() + 1);
        for (bench, view) in self.bench.benches.iter().enumerate() {
            rows.push(BenchRow::Bench(bench));
            if !self.bench.unfolded.contains(&view.id) {
                continue;
            }
            let Some(tree) = self.bench_tree(&view.id) else {
                continue;
            };
            let list = self.bench_agent_list(tree.archived);
            rows.extend(tree.agents.map(|index| {
                BenchRow::Agent(BenchAgent {
                    bench,
                    archived: tree.archived,
                    index,
                    top: tree.top,
                    arm: !list[index].bench_run,
                })
            }));
        }
        rows.push(BenchRow::New);
        rows
    }

    /// The listed agents, or the archived ones.
    #[must_use]
    pub(crate) fn bench_agent_list(&self, archived: bool) -> &[AgentSummary] {
        if archived {
            &self.archived
        } else {
            &self.agents
        }
    }

    /// The agent `agent` finds.
    #[must_use]
    pub(crate) fn bench_agent(&self, agent: BenchAgent) -> &AgentSummary {
        &self.bench_agent_list(agent.archived)[agent.index]
    }

    /// Where the agents of benchmark `id` are: under its own agent, in the
    /// projects list or else the Archive, up to the next agent no deeper
    /// than it, or the next project.
    fn bench_tree(&self, id: &str) -> Option<BenchTree> {
        [false, true].into_iter().find_map(|archived| {
            let list = self.bench_agent_list(archived);
            let at = list.iter().position(|agent| agent.id == id)?;
            let top = list[at].depth;
            let end = (at + 1..list.len())
                .find(|&i| list[i].depth <= top || (!archived && starts_project(list, i)))
                .unwrap_or(list.len());
            Some(BenchTree {
                archived,
                top,
                agents: at + 1..end,
            })
        })
    }

    /// Whether benchmark `index` has agents listed to unfold over.
    #[must_use]
    pub(crate) fn bench_has_agents(&self, index: usize) -> bool {
        self.bench
            .benches
            .get(index)
            .and_then(|view| self.bench_tree(&view.id))
            .is_some_and(|tree| !tree.agents.is_empty())
    }

    /// Whether benchmark `index` is unfolded over its agents.
    #[must_use]
    pub(crate) fn bench_unfolded(&self, index: usize) -> bool {
        self.bench
            .benches
            .get(index)
            .is_some_and(|view| self.bench.unfolded.contains(&view.id))
    }

    /// Fold benchmark `index` over its agents, or unfold it. A highlight
    /// on one of its agents moves onto it.
    fn toggle_bench_fold(&mut self, index: usize) {
        let Some(id) = self.bench.benches.get(index).map(|view| view.id.clone()) else {
            return;
        };
        if self.bench.unfolded.insert(id.clone()) {
            return;
        }
        self.bench.unfolded.remove(&id);
        if matches!(&self.bench.highlight, Some(BenchMode::Agent { bench, .. }) if *bench == id) {
            self.bench.highlight = Some(BenchMode::Detail(id));
        }
    }

    /// What Enter on row `row` of `rows` shows; `None` on an arm.
    fn bench_row_mode(&self, rows: &[BenchRow], row: usize) -> Option<BenchMode> {
        match *rows.get(row)? {
            BenchRow::Bench(index) => Some(BenchMode::Detail(self.bench.benches[index].id.clone())),
            BenchRow::Agent(agent) if agent.arm => None,
            BenchRow::Agent(agent) => Some(BenchMode::Agent {
                bench: self.bench.benches[agent.bench].id.clone(),
                agent: self.bench_agent(agent).id.clone(),
            }),
            BenchRow::New => Some(BenchMode::Form),
        }
    }

    /// The row of `rows` that shows `mode`. An agent folded away has its
    /// benchmark's.
    fn bench_row_of(&self, rows: &[BenchRow], mode: &BenchMode) -> Option<usize> {
        let bench_row = |id: &str| {
            rows.iter().position(
                |row| matches!(*row, BenchRow::Bench(index) if self.bench.benches[index].id == id),
            )
        };
        match mode {
            BenchMode::Detail(id) => bench_row(id),
            BenchMode::Form => rows.iter().position(|row| *row == BenchRow::New),
            BenchMode::Agent { bench, agent } => rows
                .iter()
                .position(|row| {
                    matches!(*row, BenchRow::Agent(at) if !at.arm && self.bench_agent(at).id == *agent)
                })
                .or_else(|| bench_row(bench)),
        }
    }

    /// The highlighted row of `rows`: the first until one is picked.
    fn highlighted_bench_row(&self, rows: &[BenchRow]) -> usize {
        self.bench
            .highlight
            .as_ref()
            .and_then(|mode| self.bench_row_of(rows, mode))
            .unwrap_or(0)
    }

    /// The listed benchmark the highlight is on, or on an agent of.
    pub(super) fn highlighted_bench(&self) -> Option<&str> {
        let id = match self.bench.highlight.as_ref()? {
            BenchMode::Detail(id) | BenchMode::Agent { bench: id, .. } => id,
            BenchMode::Form => return None,
        };
        self.bench.find(id).map(|view| view.id.as_str())
    }

    /// Highlighted row of the list, an index into [`Self::bench_rows`].
    #[must_use]
    pub(crate) fn selected_bench(&self) -> usize {
        self.highlighted_bench_row(&self.bench_rows())
    }

    /// Row of the list for what the chat panel shows: its benchmark's,
    /// its agent's (or the benchmark's while folded over it), or
    /// "+ New bench" for the form.
    #[must_use]
    pub(crate) fn shown_bench_row(&self) -> Option<usize> {
        let mode = self.bench.mode.as_ref()?;
        self.bench_row_of(&self.bench_rows(), mode)
    }

    /// First visible row of the list.
    #[must_use]
    pub(crate) const fn bench_list_scroll(&self) -> usize {
        self.bench.list_scroll
    }

    /// Keep the list's highlight inside rows `visible_rows` tall.
    /// "+ New bench" is pinned below them, so it never scrolls.
    pub(crate) fn sync_bench_list_scroll(&mut self, visible_rows: usize) {
        let rows = self.bench_rows();
        let selected = self.highlighted_bench_row(&rows);
        self.bench.list_scroll = follow_selection(
            self.bench.list_scroll,
            selected,
            visible_rows,
            rows.len() - 1,
        );
    }

    /// Record where the list's rows were drawn this frame.
    pub(crate) fn set_bench_rows_area(&mut self, area: Rect) {
        self.bench.rows_area = Some(area);
    }

    /// Record where "+ New bench" was drawn this frame.
    pub(crate) fn set_new_bench_area(&mut self, area: Rect) {
        self.bench.new_bench_area = Some(area);
    }

    /// Row of the list drawn at a screen cell, an index into
    /// [`Self::bench_rows`]; `None` on an arm, which takes no clicks.
    pub(in crate::app) fn bench_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let rows = self.bench_rows();
        let new_bench = rows.len() - 1;
        if contains(self.bench.new_bench_area, column, row) {
            return Some(new_bench);
        }
        let area = self
            .bench
            .rows_area
            .filter(|&area| contains(Some(area), column, row))?;
        let index = self.bench.list_scroll + usize::from(row - area.y);
        (index < new_bench && takes_highlight(rows[index])).then_some(index)
    }

    /// Whether `column` is on the arrow before a benchmark's status.
    fn on_bench_fold(&self, column: u16) -> bool {
        self.bench
            .rows_area
            .is_some_and(|area| column.saturating_sub(area.x) < FOLD_COLUMNS)
    }

    /// Clicking a benchmark's arrow folds or unfolds it. Clicking a row
    /// otherwise highlights it and shows it, as Enter does, but a
    /// benchmark leaves the keys on the list.
    pub(in crate::app) fn click_bench_row(&mut self, column: u16, row: u16) {
        let Some(index) = self.bench_row_at(column, row) else {
            return;
        };
        let rows = self.bench_rows();
        self.bench.highlight = self.bench_row_mode(&rows, index);
        match rows[index] {
            BenchRow::Bench(bench)
                if self.on_bench_fold(column) && self.bench_has_agents(bench) =>
            {
                self.toggle_bench_fold(bench);
            }
            _ => self.activate_bench_row(false),
        }
    }

    /// Move the highlight `delta` rows, past the arms, as the wheel does.
    pub(in crate::app) fn move_bench_highlight(&mut self, delta: isize) {
        let rows = self.bench_rows();
        let target = self
            .highlighted_bench_row(&rows)
            .saturating_add_signed(delta)
            .min(rows.len() - 1);
        let row = landing(&rows, target, delta > 0);
        self.bench.highlight = self.bench_row_mode(&rows, row);
    }

    /// Keys of the BENCH list. Returns whether the key was used.
    pub(in crate::app) fn handle_bench_list_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Up => self.move_bench_highlight(-1),
            KeyCode::Down => self.move_bench_highlight(1),
            KeyCode::PageUp => self.move_bench_highlight(-PAGE_STEP),
            KeyCode::PageDown => self.move_bench_highlight(PAGE_STEP),
            KeyCode::Home => self.move_bench_highlight(isize::MIN),
            KeyCode::End => self.move_bench_highlight(isize::MAX),
            KeyCode::Enter => self.activate_bench_row(true),
            KeyCode::Char(' ') => self.fold_highlighted_bench(),
            KeyCode::Char('n') => {
                self.bench.highlight = Some(BenchMode::Form);
                self.activate_bench_row(true);
            }
            KeyCode::Char('r') => self.send_bench(BenchRequest::List),
            KeyCode::Char('c') => self.send_bench(BenchRequest::Cancel),
            KeyCode::Left | KeyCode::Right => self.shift_agents_tab(key.code),
            _ => return false,
        }
        true
    }

    /// Space: fold or unfold the highlighted benchmark, or fold the one
    /// the highlighted agent works for.
    fn fold_highlighted_bench(&mut self) {
        let rows = self.bench_rows();
        match rows.get(self.highlighted_bench_row(&rows)) {
            Some(&BenchRow::Bench(bench)) => self.toggle_bench_fold(bench),
            Some(&BenchRow::Agent(agent)) => self.toggle_bench_fold(agent.bench),
            _ => {}
        }
    }

    /// Show the highlighted row. A benchmark takes the keys on `enter`;
    /// an agent's chat and the form always take them.
    fn activate_bench_row(&mut self, enter: bool) {
        let rows = self.bench_rows();
        let Some(mode) = self.bench_row_mode(&rows, self.highlighted_bench_row(&rows)) else {
            return;
        };
        let takes_keys = enter || !matches!(mode, BenchMode::Detail(_));
        self.show_bench(mode);
        if takes_keys {
            self.focus = PanelFocus::Chat;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{AgentsTab, ChatTab};
    use crate::events::{BenchArm, BenchRun, BenchView, UiCommand, UiEvent};
    use crossterm::event::KeyModifiers;
    use tokio::sync::mpsc;

    fn agent(id: &str, depth: usize, bench_run: bool) -> AgentSummary {
        AgentSummary {
            id: id.to_string(),
            name: id.to_string(),
            depth,
            bench_run,
            ..AgentSummary::default()
        }
    }

    fn view(id: &str) -> BenchView {
        BenchView {
            id: id.to_string(),
            root: "lead".to_string(),
            name: format!("Compare {id}"),
            title: format!("work {id}"),
            arms: [BenchArm::default(), BenchArm::default()],
            ..BenchView::default()
        }
    }

    /// `lead`, under it `c1`'s agent, under that `arm-a` over runs `a-1`,
    /// which spawned `scout`, and `a-2`, and `arm-b` over `b-1`; then
    /// `other` in a project of its own.
    fn agents() -> Vec<AgentSummary> {
        vec![
            agent("lead", 0, false),
            agent("c1", 1, false),
            agent("arm-a", 2, false),
            agent("a-1", 3, true),
            agent("scout", 4, true),
            agent("a-2", 3, true),
            agent("arm-b", 2, false),
            agent("b-1", 3, true),
            agent("other", 0, false),
        ]
    }

    /// [`agents`], `lead` on screen in PROJECTS, and benchmarks `c1` and
    /// `c2` listed, `c2` the newest, with no agents; and what the app
    /// sends.
    fn app() -> (App, mpsc::Receiver<UiEvent>) {
        let (tx, rx) = mpsc::channel(64);
        let mut app = App::new().with_event_sender(tx);
        app.process_command(UiCommand::SetAgents(agents()));
        app.process_command(UiCommand::SetActiveAgent("lead".to_string()));
        app.process_command(UiCommand::BenchList {
            benches: vec![view("c1"), view("c2")],
        });
        (app, rx)
    }

    /// [`app`] in BENCH with the keys on the list, `c1` unfolded and
    /// highlighted.
    fn unfolded() -> (App, mpsc::Receiver<UiEvent>) {
        let (mut app, rx) = app();
        app.show_agents_tab(AgentsTab::Bench);
        app.focus = PanelFocus::Swarm;
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Char(' ')));
        (app, rx)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    /// The list's rows by id: a benchmark's, an agent's (an arm's in
    /// brackets), and `+` for "+ New bench".
    fn ids(app: &App) -> Vec<String> {
        app.bench_rows()
            .into_iter()
            .map(|row| match row {
                BenchRow::Bench(index) => app.bench.benches[index].id.clone(),
                BenchRow::Agent(agent) if agent.arm => format!("[{}]", app.bench_agent(agent).id),
                BenchRow::Agent(agent) => app.bench_agent(agent).id.clone(),
                BenchRow::New => "+".to_string(),
            })
            .collect()
    }

    /// The id of the highlighted row, as [`ids`] writes it.
    fn highlighted(app: &App) -> String {
        ids(app).swap_remove(app.selected_bench())
    }

    fn agent_mode(agent: &str) -> BenchMode {
        BenchMode::Agent {
            bench: "c1".to_string(),
            agent: agent.to_string(),
        }
    }

    #[test]
    fn a_benchmark_unfolds_over_its_arms_runs_and_the_agents_they_spawned() {
        let (mut app, _rx) = app();
        app.show_agents_tab(AgentsTab::Bench);
        app.focus = PanelFocus::Swarm;
        assert_eq!(ids(&app), ["c2", "c1", "+"], "folded at first");
        assert!(!app.bench_has_agents(0), "c2 has none listed");
        assert!(app.bench_has_agents(1));

        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Char(' ')));
        assert!(app.bench_unfolded(1));
        assert_eq!(
            ids(&app),
            ["c2", "c1", "[arm-a]", "a-1", "scout", "a-2", "[arm-b]", "b-1", "+"]
        );
        app.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(ids(&app), ["c2", "c1", "+"]);

        // Archived, they unfold from the Archive, where `c1` is at the top.
        let mut listed = agents();
        let archived: Vec<_> = listed
            .drain(1..8)
            .map(|agent| AgentSummary {
                depth: agent.depth - 1,
                ..agent
            })
            .collect();
        app.process_command(UiCommand::SetAgents(listed));
        app.process_command(UiCommand::SetArchivedAgents(archived));
        app.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(
            ids(&app),
            ["c2", "c1", "[arm-a]", "a-1", "scout", "a-2", "[arm-b]", "b-1", "+"]
        );
        assert!(app.bench_rows().iter().any(|row| matches!(
            row,
            BenchRow::Agent(BenchAgent {
                archived: true,
                top: 0,
                ..
            })
        )));
    }

    #[test]
    fn the_highlight_passes_over_the_arms_and_stays_on_its_row() {
        let (mut app, _rx) = unfolded();
        assert_eq!(highlighted(&app), "c1");
        app.handle_key(key(KeyCode::Down));
        assert_eq!(highlighted(&app), "a-1", "past arm-a");
        for _ in 0..3 {
            app.handle_key(key(KeyCode::Down));
        }
        assert_eq!(highlighted(&app), "b-1", "past arm-b");
        app.handle_key(key(KeyCode::Up));
        assert_eq!(highlighted(&app), "a-2");

        // A run starting above it does not move it.
        let mut more = agents();
        more.insert(5, agent("a-3", 3, true));
        app.process_command(UiCommand::SetAgents(more));
        assert_eq!(highlighted(&app), "a-2");

        app.handle_key(key(KeyCode::End));
        assert_eq!(highlighted(&app), "+");
        app.handle_key(key(KeyCode::Home));
        assert_eq!(highlighted(&app), "c2");
    }

    #[test]
    fn space_on_an_agent_folds_its_benchmark_onto_it() {
        let (mut app, _rx) = unfolded();
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        assert_eq!(highlighted(&app), "scout");
        app.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(ids(&app), ["c2", "c1", "+"]);
        assert_eq!(highlighted(&app), "c1");
        app.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(highlighted(&app), "c1", "unfolding leaves it there");
    }

    #[test]
    fn an_agent_shows_in_bench_with_its_tabs_and_takes_the_keys() {
        let (mut app, mut rx) = unfolded();
        while rx.try_recv().is_ok() {}
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.agents_tab(), AgentsTab::Bench);
        assert_eq!(app.bench.mode, Some(agent_mode("a-1")));
        assert_eq!(app.active_agent_id(), "a-1", "on screen at once");
        assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "a-1"));
        assert_eq!(app.focus(), PanelFocus::Chat, "typing goes to it");
        assert!(!app.no_agent_on_screen());
        assert_eq!(app.app_chat_title(), None, "the agent's own header");
        assert!(!app.chat_tab_layout(80).cells.is_empty(), "with its tabs");
        app.show_chat_tab(ChatTab::Log);
        assert_eq!(app.chat_tab(), ChatTab::Log);
        assert_eq!(app.shown_bench_row(), Some(3), "its row is lit");

        // The session confirming it keeps BENCH in front.
        app.process_command(UiCommand::SetActiveAgent("a-1".to_string()));
        assert_eq!(app.agents_tab(), AgentsTab::Bench);

        // Esc takes LOG back to CHAT, then hands the keys to the list.
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.chat_tab(), ChatTab::Chat);
        assert_eq!(app.focus(), PanelFocus::Chat);
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.focus(), PanelFocus::Swarm);

        // Any other agent coming on screen brings PROJECTS back.
        app.process_command(UiCommand::SetActiveAgent("other".to_string()));
        assert_eq!(app.agents_tab(), AgentsTab::Projects);
    }

    #[test]
    fn a_benchmark_and_its_agents_take_turns_with_the_chat_tabs() {
        let (mut app, _rx) = app();
        app.show_chat_tab(ChatTab::Graph);
        app.show_agents_tab(AgentsTab::Bench);
        app.focus = PanelFocus::Swarm;
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Char(' ')));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.chat_tab(), ChatTab::Chat, "a-1 starts on CHAT");
        app.show_chat_tab(ChatTab::Log);

        // A benchmark puts the agent's tabs away, and the agent coming
        // back has them back.
        app.focus = PanelFocus::Swarm;
        app.handle_key(key(KeyCode::Home));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.bench.mode, Some(BenchMode::Detail("c2".to_string())));
        assert_eq!(app.chat_tab(), ChatTab::Chat);
        assert!(app.chat_tab_layout(80).cells.is_empty());
        assert!(app.no_agent_on_screen());
        app.focus = PanelFocus::Swarm;
        for _ in 0..2 {
            app.handle_key(key(KeyCode::Down));
        }
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.bench.mode, Some(agent_mode("a-1")));
        assert_eq!(app.chat_tab(), ChatTab::Log);

        // Leaving and coming back: each app shows what it was left on,
        // on its tab.
        app.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::ALT));
        assert_eq!(app.active_agent_id(), "lead");
        assert_eq!(app.chat_tab(), ChatTab::Graph);
        app.handle_key(KeyEvent::new(KeyCode::Char('4'), KeyModifiers::ALT));
        assert_eq!(app.agents_tab(), AgentsTab::Bench);
        assert_eq!(app.active_agent_id(), "a-1");
        assert_eq!(app.chat_tab(), ChatTab::Log);

        // CHANNELS has only CHAT; the agent gets LOG back after it.
        app.show_agents_tab(AgentsTab::Channels);
        assert_eq!(app.chat_tab(), ChatTab::Chat);
        app.show_agents_tab(AgentsTab::Bench);
        assert_eq!(app.chat_tab(), ChatTab::Log);
    }

    #[test]
    fn the_form_opened_over_an_agent_goes_back_to_it() {
        let (mut app, _rx) = unfolded();
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        app.show_chat_tab(ChatTab::Log);
        app.focus = PanelFocus::Swarm;
        app.handle_key(key(KeyCode::Char('n')));
        assert_eq!(app.bench.mode, Some(BenchMode::Form));
        assert_eq!(app.chat_tab(), ChatTab::Chat);

        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.bench.mode, Some(agent_mode("a-1")));
        assert_eq!(app.chat_tab(), ChatTab::Log);
    }

    #[test]
    fn coming_back_to_bench_shows_a_benchmark_for_an_agent_gone() {
        let (mut app, _rx) = unfolded();
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        app.show_agents_tab(AgentsTab::Projects);
        let mut gone = agents();
        gone.retain(|agent| agent.id != "a-1");
        app.process_command(UiCommand::SetAgents(gone));

        app.show_agents_tab(AgentsTab::Bench);
        assert_eq!(app.bench.mode, Some(BenchMode::Detail("c1".to_string())));
        assert_eq!(app.active_agent_id(), "lead");
        assert!(app.chat_tab_layout(80).cells.is_empty());
    }

    #[test]
    fn a_click_folds_on_the_arrow_and_shows_elsewhere() {
        let (mut app, _rx) = app();
        app.show_agents_tab(AgentsTab::Bench);
        app.focus = PanelFocus::Swarm;
        app.set_bench_rows_area(Rect::new(1, 2, 40, 12));
        app.set_new_bench_area(Rect::new(1, 14, 40, 1));
        // c1 is on screen row 3: its arrow unfolds it, and shows nothing.
        app.click_bench_row(2, 3);
        assert!(app.bench_unfolded(1));
        assert_eq!(app.bench.mode, Some(BenchMode::Detail("c2".to_string())));
        assert_eq!(app.bench_row_at(10, 4), None, "arm-a takes no click");

        app.click_bench_row(10, 3);
        assert_eq!(app.bench.mode, Some(BenchMode::Detail("c1".to_string())));
        assert_eq!(
            app.focus(),
            PanelFocus::Swarm,
            "a benchmark leaves the keys"
        );
        app.click_bench_row(10, 5);
        assert_eq!(app.bench.mode, Some(agent_mode("a-1")));
        assert_eq!(app.focus(), PanelFocus::Chat, "an agent takes them");

        // c2 has no agents: its arrow's columns show it.
        app.click_bench_row(2, 2);
        assert_eq!(app.bench.mode, Some(BenchMode::Detail("c2".to_string())));
    }

    #[test]
    fn l_shows_a_run_s_log_under_its_benchmark() {
        let (mut app, _rx) = app();
        let mut c1 = view("c1");
        c1.status = crate::events::BenchStatus::Done;
        c1.runs = vec![BenchRun {
            name: "a-2".to_string(),
            agent_id: "a-2".to_string(),
            ..BenchRun::default()
        }];
        app.process_command(UiCommand::BenchUpsert(Box::new(c1)));
        app.show_agents_tab(AgentsTab::Bench);
        app.show_bench(BenchMode::Detail("c1".to_string()));
        app.focus = PanelFocus::Chat;
        app.handle_key(key(KeyCode::Right));
        app.handle_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT));
        assert_eq!(app.agents_tab(), AgentsTab::Bench);
        assert_eq!(app.bench.mode, Some(agent_mode("a-2")));
        assert_eq!(app.active_agent_id(), "a-2");
        assert_eq!(app.chat_tab(), ChatTab::Log);
        assert!(app.bench_unfolded(1), "its row shows");
        assert_eq!(highlighted(&app), "a-2");
    }

    /// The screen at `width` columns, a line per row.
    fn screen(app: &mut App, width: u16) -> Vec<String> {
        use ratatui::{backend::TestBackend, Terminal};
        let mut terminal = Terminal::new(TestBackend::new(width, 30)).unwrap();
        let theme = crate::Theme::cyber();
        terminal
            .draw(|frame| crate::renderer::render(frame, app, &theme))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let width = usize::from(buffer.area.width);
        buffer
            .content
            .chunks(width)
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
            .collect()
    }

    #[test]
    fn an_unfolded_benchmark_hangs_its_agents_from_its_name() {
        let (mut app, _rx) = unfolded();
        let rows = screen(&mut app, 140);
        let at = |text: &str| {
            rows.iter()
                .position(|row| row.contains(text))
                .unwrap_or_else(|| panic!("no {text}:\n{}", rows.join("\n")))
        };
        let before = |row: usize, text: &str| &rows[row][..rows[row].find(text).unwrap()];
        let bench = at("Compare c1");
        assert!(before(bench, "Compare c1").contains('▾'), "{}", rows[bench]);
        // The list's c2, above it: no arrow without agents.
        let lead = before(bench - 1, "Compare c2");
        assert!(!lead.contains('▸') && !lead.contains('▾'), "{lead}");
        for (row, guides) in [
            (bench + 1, "├── arm-a"),
            (bench + 2, "│   ├── a-1"),
            (bench + 3, "│   │   └── scout"),
            (bench + 4, "│   └── a-2"),
            (bench + 5, "└── arm-b"),
            (bench + 6, "    └── b-1"),
        ] {
            assert!(rows[row].contains(guides), "{guides}:\n{}", rows.join("\n"));
        }
        assert_eq!(
            before(bench + 1, "├── arm-a").chars().count(),
            before(bench, "Compare c1").chars().count(),
            "the lines drop from the benchmark's name"
        );
    }
}
