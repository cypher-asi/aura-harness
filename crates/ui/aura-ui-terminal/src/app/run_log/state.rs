//! The LOG tab's state: parking it per agent, its panels, folds,
//! selection and copies.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use super::{
    fade_progress, LogPanel, LogRow, LogState, ParkedLog, COPIED_FLASH, LIVE_EVERY, PARKED_AGENTS,
    RELOAD_GIVE_UP,
};
use crate::events::{LogRunView, LogStepView, LogTurnView, LogView};
use ratatui::layout::Rect;

impl LogState {
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Open or shut the tab. Coming onto it holds the log on hand back
    /// until a fresh one arrives.
    pub(in crate::app) fn set_open(&mut self, open: bool) {
        if open && !self.open {
            self.hold();
        }
        self.open = open;
    }

    /// Stop drawing the log on hand until the next one arrives.
    pub(super) fn hold(&mut self) {
        self.held = self.view.is_some() || self.notice.is_some();
        self.revealed_at = None;
    }

    /// Make the state the one of `agent_id`: park the state of the agent
    /// it belonged to and bring back what `agent_id`'s tab showed, or
    /// start empty. Nothing changes when it already is `agent_id`'s.
    pub fn show_agent(&mut self, agent_id: &str) {
        if self.agent == agent_id {
            return;
        }
        let previous = std::mem::replace(&mut self.agent, agent_id.to_string());
        let coming = self.take_parked(agent_id).unwrap_or_default();
        let leaving = ParkedLog {
            view: self.view.take(),
            notice: self.notice.take(),
            expanded: std::mem::take(&mut self.expanded),
            raw: std::mem::take(&mut self.raw),
            selected: self.selected.take(),
            scroll: std::mem::take(&mut self.scroll),
        };
        self.parked.retain(|(id, _)| id != &previous);
        if leaving.view.is_some() || leaving.notice.is_some() {
            self.parked.push((previous, leaving));
        }
        if self.parked.len() > PARKED_AGENTS {
            let extra = self.parked.len() - PARKED_AGENTS;
            self.parked.drain(..extra);
        }
        self.view = coming.view;
        self.notice = coming.notice;
        self.expanded = coming.expanded;
        self.raw = coming.raw;
        self.selected = coming.selected;
        self.scroll = coming.scroll;
        self.follow = false;
        self.loading_since = None;
        self.hold();
    }

    /// The state parked for `agent_id`, taken out of the cache.
    pub(super) fn take_parked(&mut self, agent_id: &str) -> Option<ParkedLog> {
        let at = self.parked.iter().position(|(id, _)| id == agent_id)?;
        Some(self.parked.remove(at).1)
    }

    /// The log to draw for `agent_id`: the loaded one, when it is that
    /// agent's and not held back for a fresh one.
    #[must_use]
    pub fn shown(&self, agent_id: &str) -> Option<&LogView> {
        self.view
            .as_ref()
            .filter(|v| !self.held && v.agent_id == agent_id)
    }

    /// Whether the log or notice on hand waits for a fresh one before it
    /// is drawn.
    #[must_use]
    pub const fn is_held(&self) -> bool {
        self.held
    }

    /// Whether a log was asked for and hasn't arrived.
    #[must_use]
    pub const fn is_loading(&self) -> bool {
        self.loading_since.is_some()
    }

    /// How far a log arriving has faded in, as milliseconds of
    /// [`FADE_IN`], while it still is.
    #[must_use]
    pub fn fading(&self) -> Option<(usize, usize)> {
        fade_progress(self.revealed_at)
    }

    /// A step was recorded: reload the log if it is open.
    pub(in crate::app) fn mark_stale(&mut self) {
        self.stale |= self.open;
    }

    /// Whether to reload the log now: it is open, a step was recorded
    /// since it was asked for, and it was asked for long enough ago.
    pub(super) fn reload_due(&self) -> bool {
        let since = self.requested.map(|at| at.elapsed());
        self.open
            && self.stale
            && since.is_none_or(|since| {
                since >= LIVE_EVERY && (!self.is_loading() || since >= RELOAD_GIVE_UP)
            })
    }

    /// The run with id `run_id`.
    #[must_use]
    pub fn run(&self, run_id: &str) -> Option<&LogRunView> {
        self.view.as_ref()?.runs.iter().find(|r| r.run_id == run_id)
    }

    /// Turn `turn` of run `run_id`.
    #[must_use]
    pub fn turn(&self, run_id: &str, turn: u32) -> Option<&LogTurnView> {
        self.run(run_id)?.turns.iter().find(|t| t.turn == turn)
    }

    /// Step `seq` of run `run_id`.
    #[must_use]
    pub fn step(&self, run_id: &str, seq: u64) -> Option<&LogStepView> {
        self.run(run_id)?
            .turns
            .iter()
            .flat_map(|t| &t.steps)
            .find(|s| s.seq == seq)
    }

    /// Whether `panel` is open. The run asked for always is.
    #[must_use]
    pub fn is_expanded(&self, panel: &LogPanel) -> bool {
        self.is_root(panel) || self.expanded.contains(panel)
    }

    /// Whether `panel` is the run asked for, which has no row.
    pub(super) fn is_root(&self, panel: &LogPanel) -> bool {
        matches!(panel, LogPanel::Run(id)
            if self.view.as_ref().and_then(LogView::root).is_some_and(|r| &r.run_id == id))
    }

    /// Whether `panel` has anything to open.
    #[must_use]
    pub fn opens(&self, panel: &LogPanel) -> bool {
        match panel {
            LogPanel::Run(_) | LogPanel::Details(_) => true,
            LogPanel::Turn(run, turn) => self.turn(run, *turn).is_some_and(|t| !t.steps.is_empty()),
            LogPanel::Step(run, seq) => self
                .step(run, *seq)
                .is_some_and(|s| !s.fields.is_empty() || !s.sections.is_empty()),
        }
    }

    /// The panel headers on screen, top to bottom. The run asked for has
    /// no row: its panels are the top level.
    #[must_use]
    pub fn rows(&self) -> Vec<LogRow> {
        let mut rows = Vec::new();
        if let Some(view) = &self.view {
            if let Some(root) = view.root() {
                let mut seen = HashSet::from([0]);
                self.push_contents(view, root, 0, &mut rows, &mut seen);
            }
        }
        rows
    }

    /// The row of run `index` of `view` at `depth`, and its panels below
    /// it when it is open.
    pub(super) fn push_run(
        &self,
        view: &LogView,
        index: usize,
        depth: usize,
        rows: &mut Vec<LogRow>,
        seen: &mut HashSet<usize>,
    ) {
        let Some(run) = view.runs.get(index) else {
            return;
        };
        if !seen.insert(index) {
            return;
        }
        let panel = LogPanel::Run(run.run_id.clone());
        let open = self.is_expanded(&panel);
        rows.push(LogRow { panel, depth });
        if open {
            self.push_contents(view, run, depth + 1, rows, seen);
        }
    }

    /// The panels of `run` at `depth`: its details, its turns with their
    /// steps, and the runs below it.
    pub(super) fn push_contents(
        &self,
        view: &LogView,
        run: &LogRunView,
        depth: usize,
        rows: &mut Vec<LogRow>,
        seen: &mut HashSet<usize>,
    ) {
        let id = &run.run_id;
        rows.push(LogRow {
            panel: LogPanel::Details(id.clone()),
            depth,
        });
        for turn in &run.turns {
            let panel = LogPanel::Turn(id.clone(), turn.turn);
            let open = self.is_expanded(&panel);
            rows.push(LogRow { panel, depth });
            if !open {
                continue;
            }
            for step in &turn.steps {
                rows.push(LogRow {
                    panel: LogPanel::Step(id.clone(), step.seq),
                    depth: depth + 1,
                });
                for &child in &step.spawned {
                    self.push_run(view, child, depth + 2, rows, seen);
                }
            }
            for &child in &turn.spawned {
                self.push_run(view, child, depth + 1, rows, seen);
            }
        }
        for &child in &run.runs {
            self.push_run(view, child, depth, rows, seen);
        }
    }

    /// Take `view` in: a new run opens on its last turn, its details
    /// shut; the same run again keeps what was open and opens a new turn.
    /// A log coming where none was drawn, or one held back, fades in.
    pub(super) fn load(&mut self, view: LogView) {
        self.loading_since = None;
        self.notice = None;
        if self.shown(&view.agent_id).is_none() {
            self.revealed_at = Some(Instant::now());
        }
        self.held = false;
        self.agent.clone_from(&view.agent_id);
        let same = self
            .view
            .as_ref()
            .and_then(LogView::root)
            .zip(view.root())
            .is_some_and(|(old, new)| old.run_id == new.run_id);
        let last_turn = |view: &LogView| view.root().and_then(|r| r.turns.last()).map(|t| t.turn);
        let rows = self.rows();
        let at_end = same
            && self.selected.is_some()
            && rows.last().map(|r| &r.panel) == self.selected.as_ref();
        if same {
            let before = self.view.as_ref().and_then(last_turn);
            if let (Some(root), Some(turn)) = (view.root(), last_turn(&view)) {
                if before != Some(turn) {
                    self.expanded
                        .insert(LogPanel::Turn(root.run_id.clone(), turn));
                }
            }
        } else {
            self.expanded.clear();
            self.raw.clear();
            self.scroll = 0;
            self.selected = None;
            if let (Some(root), Some(turn)) = (view.root(), last_turn(&view)) {
                self.expanded
                    .insert(LogPanel::Turn(root.run_id.clone(), turn));
            }
        }
        self.view = Some(view);
        if at_end {
            // Watching the newest step: keep watching as steps come in.
            let rows = self.rows();
            self.select_at(&rows, rows.len().saturating_sub(1));
        }
        self.keep_selection();
    }

    /// Keep the selection on a panel that is on screen.
    pub(super) fn keep_selection(&mut self) {
        let rows = self.rows();
        let shown = self
            .selected
            .as_ref()
            .is_some_and(|s| rows.iter().any(|r| &r.panel == s));
        if !shown {
            self.selected = rows.first().map(|r| r.panel.clone());
        }
    }

    pub(super) fn selected_index(&self, rows: &[LogRow]) -> usize {
        self.selected
            .as_ref()
            .and_then(|s| rows.iter().position(|r| &r.panel == s))
            .unwrap_or(0)
    }

    pub(super) fn select_at(&mut self, rows: &[LogRow], index: usize) {
        if let Some(row) = rows.get(index.min(rows.len().saturating_sub(1))) {
            self.selected = Some(row.panel.clone());
            self.follow = true;
        }
    }

    pub(super) fn move_by(&mut self, delta: isize) {
        let rows = self.rows();
        let at = self.selected_index(&rows);
        let to = at.saturating_add_signed(delta);
        self.select_at(&rows, to);
    }

    /// Open or shut `panel`; the run asked for stays open.
    pub(crate) fn toggle(&mut self, panel: &LogPanel) {
        if self.is_root(panel) {
            return;
        }
        if !self.expanded.remove(panel) && self.opens(panel) {
            self.expanded.insert(panel.clone());
        }
        self.keep_selection();
    }

    /// Open the selected panel, else go to its first panel inside.
    pub(super) fn open_or_enter(&mut self) {
        let Some(panel) = self.selected.clone() else {
            return;
        };
        if self.opens(&panel) && !self.is_expanded(&panel) {
            self.expanded.insert(panel);
        } else {
            self.move_by(1);
        }
    }

    /// Shut the selected panel, else go to the panel it is in.
    pub(super) fn close_or_leave(&mut self) {
        let Some(panel) = self.selected.clone() else {
            return;
        };
        if !self.is_root(&panel) && self.expanded.remove(&panel) {
            return;
        }
        let rows = self.rows();
        let at = self.selected_index(&rows);
        let depth = rows.get(at).map_or(0, |r| r.depth);
        let parent = rows
            .get(..at)
            .and_then(|above| above.iter().rposition(|r| r.depth < depth));
        if let Some(parent) = parent {
            self.select_at(&rows, parent);
        }
    }

    /// Open every panel, the runs of every subagent included.
    pub(super) fn expand_all(&mut self) {
        let Some(view) = &self.view else {
            return;
        };
        let mut all = Vec::new();
        for run in &view.runs {
            let id = &run.run_id;
            all.push(LogPanel::Run(id.clone()));
            all.push(LogPanel::Details(id.clone()));
            for turn in &run.turns {
                all.push(LogPanel::Turn(id.clone(), turn.turn));
                for step in &turn.steps {
                    all.push(LogPanel::Step(id.clone(), step.seq));
                }
            }
        }
        self.expanded.extend(all);
    }

    /// Shut every panel; the run asked for stays open.
    pub(super) fn collapse_all(&mut self) {
        self.expanded.clear();
        self.keep_selection();
        self.follow = true;
    }

    /// The JSON of `panel`: a step's, a turn's, or its run's.
    #[must_use]
    pub fn json_of(&self, panel: &LogPanel) -> Option<&str> {
        match panel {
            LogPanel::Step(run, seq) => self.step(run, *seq).map(|s| s.json.as_str()),
            LogPanel::Turn(run, turn) => self.turn(run, *turn).map(|t| t.json.as_str()),
            other => self.run(other.run_id()).map(|r| r.json.as_str()),
        }
    }

    /// Scroll by `lines`, down when positive.
    pub(in crate::app) fn scroll_by(&mut self, lines: isize) {
        self.scroll = self.scroll.saturating_add_signed(lines);
        self.follow = false;
    }

    /// Where the lines of the last frame were and what they belong to.
    pub(crate) fn set_hits(&mut self, hits: Vec<(u16, LogPanel)>, copy_button: Option<Rect>) {
        self.hits = hits;
        self.copy_button = copy_button;
    }

    /// Where `Copy log` was drawn last frame.
    #[must_use]
    pub const fn copy_button_area(&self) -> Option<Rect> {
        self.copy_button
    }

    /// A copy from the tab went through (`ok`) or failed.
    pub(in crate::app) fn copy_finished(&mut self, ok: bool) {
        if std::mem::take(&mut self.copying) && ok {
            self.copied_at = Some(Instant::now());
        }
    }

    /// Whether a copy from the tab went through a moment ago.
    #[must_use]
    pub fn just_copied(&self) -> bool {
        self.copied_at.is_some_and(|at| at.elapsed() < COPIED_FLASH)
    }

    pub(super) fn panel_at(&self, row: u16) -> Option<LogPanel> {
        self.hits
            .iter()
            .find(|(y, _)| *y == row)
            .map(|(_, panel)| panel.clone())
    }

    /// Index of the run with id `run_id` in the view.
    #[must_use]
    pub fn run_indices(&self) -> HashMap<&str, usize> {
        self.view.as_ref().map_or_else(HashMap::new, |view| {
            view.runs
                .iter()
                .enumerate()
                .map(|(i, r)| (r.run_id.as_str(), i))
                .collect()
        })
    }
}
