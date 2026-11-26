//! The benchmark progress panel above the input: how far the benchmarks
//! of the tree on screen have got, one tab each, from any chat tab.
//!
//! It keeps every benchmark it has heard running, as last heard, whether
//! or not BENCH ever listed them: a benchmark's runs report every few
//! seconds whatever is on screen. A finished one stays, its tab saying
//! so, until the user closes it; a closed one stays hidden.
//!
//! Mouse only, so typing still goes to the input: a press on a tab picks
//! it, on its `×` closes it, on the toggle folds or opens the panel, and
//! anywhere else on it opens the benchmark in BENCH.

use super::{AgentsTab, App, BenchMode};
use crate::events::BenchView;
use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::layout::Rect;
use std::collections::HashSet;

/// Where a tab of the panel's header was drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BenchTabArea {
    /// Id of its benchmark
    pub id: String,
    pub tab: Rect,
    /// Its `×`, drawn on the picked tab only
    pub close: Option<Rect>,
}

/// Where the panel drew its parts this frame, for the mouse.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BenchPanelAreas {
    pub tabs: Vec<BenchTabArea>,
    pub toggle: Option<Rect>,
    /// The whole panel
    pub panel: Option<Rect>,
}

/// The panel's state.
#[derive(Debug, Clone, Default)]
pub(crate) struct BenchPanel {
    /// Benchmarks heard running, in the order they were first heard
    tracked: Vec<BenchView>,
    /// Ids of those the user closed
    closed: HashSet<String>,
    /// Id of the picked tab; the newest shown when it is gone
    picked: Option<String>,
    collapsed: bool,
    areas: BenchPanelAreas,
}

impl BenchPanel {
    /// Take what a benchmark is now: a new one when it runs, the one
    /// known by its id replaced. A new one gets the panel's pick.
    pub(super) fn heard(&mut self, view: &BenchView) {
        if let Some(known) = self.tracked.iter_mut().find(|b| b.id == view.id) {
            *known = view.clone();
            return;
        }
        if view.progress().phase.under_way() && !self.closed.contains(&view.id) {
            self.picked = Some(view.id.clone());
            self.tracked.push(view.clone());
        }
    }
}

impl App {
    /// Hex id of the agent at the top of the tree on screen; `None` until
    /// the agents are listed.
    pub(super) fn bench_tree_root(&self) -> Option<&str> {
        let index = self
            .agents
            .iter()
            .position(|a| a.id == self.active_agent_id)?;
        let root = (0..=index)
            .rev()
            .find(|&i| self.agents[i].depth == 0)
            .unwrap_or(index);
        Some(&self.agents[root].id)
    }

    /// The benchmarks the panel has a tab for, oldest first: those of the
    /// tree on screen, or every one before the agents are listed.
    #[must_use]
    pub(crate) fn bench_panel_views(&self) -> Vec<&BenchView> {
        let root = self.bench_tree_root();
        self.bench_panel
            .tracked
            .iter()
            .filter(|b| root.is_none_or(|root| b.root == root))
            .filter(|b| !self.bench_panel.closed.contains(&b.id))
            .collect()
    }

    /// The benchmark of the picked tab, or of the newest when none is.
    /// None in the channel app, where benchmarks are an agent feature, in
    /// BENCH, whose list has them, and with no agent on screen.
    #[must_use]
    pub(crate) fn bench_panel_shown(&self) -> Option<&BenchView> {
        if self.channel_app() || self.bench_app() || self.no_agent_on_screen() {
            return None;
        }
        let views = self.bench_panel_views();
        let picked = self.bench_panel.picked.as_deref();
        views
            .iter()
            .find(|b| Some(b.id.as_str()) == picked)
            .or_else(|| views.last())
            .copied()
    }

    #[must_use]
    pub(crate) const fn bench_panel_collapsed(&self) -> bool {
        self.bench_panel.collapsed
    }

    pub(crate) fn set_bench_panel_areas(&mut self, areas: BenchPanelAreas) {
        self.bench_panel.areas = areas;
    }

    #[cfg(test)]
    pub(crate) const fn bench_panel_areas(&self) -> &BenchPanelAreas {
        &self.bench_panel.areas
    }

    /// Close the tab of benchmark `id`. A finished one is forgotten.
    pub(crate) fn close_bench_tab(&mut self, id: &str) {
        let panel = &mut self.bench_panel;
        panel.closed.insert(id.to_string());
        panel
            .tracked
            .retain(|b| b.id != id || b.progress().phase.under_way());
        if panel.picked.as_deref() == Some(id) {
            panel.picked = None;
        }
    }

    /// Bring BENCH to the front showing benchmark `view`.
    fn open_bench_detail(&mut self, view: BenchView) {
        let id = view.id.clone();
        // Until the list asked for arrives, BENCH shows what the panel
        // last heard.
        if !self.bench.benches.iter().any(|b| b.id == id) {
            self.bench.benches.insert(0, view);
        }
        self.switch_app(AgentsTab::Bench);
        self.show_bench(BenchMode::Detail(id));
    }

    /// Take a mouse event aimed at the panel. Returns whether the panel
    /// used it: only a left press on it.
    pub(super) fn bench_panel_mouse(
        &mut self,
        kind: MouseEventKind,
        column: u16,
        row: u16,
    ) -> bool {
        if kind != MouseEventKind::Down(MouseButton::Left) {
            return false;
        }
        let areas = &self.bench_panel.areas;
        if !super::contains(areas.panel, column, row) {
            return false;
        }
        let toggle = super::contains(areas.toggle, column, row);
        let hit = areas
            .tabs
            .iter()
            .find(|t| super::contains(Some(t.tab), column, row))
            .map(|t| (t.id.clone(), super::contains(t.close, column, row)));
        self.clear_selection();
        if toggle {
            self.bench_panel.collapsed = !self.bench_panel.collapsed;
            return true;
        }
        match hit {
            Some((id, true)) => self.close_bench_tab(&id),
            Some((id, false)) => self.bench_panel.picked = Some(id),
            None => {
                if let Some(view) = self.bench_panel_shown().cloned() {
                    self.open_bench_detail(view);
                }
            }
        }
        true
    }
}
