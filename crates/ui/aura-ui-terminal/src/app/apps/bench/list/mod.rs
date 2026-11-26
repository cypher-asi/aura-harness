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
            &self.archived.items
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
mod tests;
