//! BENCH, an app of its own beside the agents: benchmarks that run the
//! same work on two arms (Gate, a pinned model, or a model per
//! difficulty) and compare what they cost and how well they did. The
//! event loop runs them as `/compare` comparisons.
//!
//! The agents panel's BENCH tab lists every benchmark, of every tree,
//! newest first, each unfolding over the agents that work for it, then
//! "+ New bench" (see `list`). The chat panel shows what is picked there.
//! A benchmark, or the form that sets a new one up (see `form`), shows
//! under a header with no tabs, and no agent is on screen: the right
//! column, the queue and the progress panel stay away, and the input runs
//! commands and fills the form's text rows but sends no message. One of a
//! benchmark's agents shows as PROJECTS would show it: its chat with all
//! its tabs, its panels, and an input that sends to it.
//!
//! - Coming to BENCH shows what it was left on, else the highlighted
//!   benchmark, else the newest. So does a list arriving while it shows
//!   nothing.
//! - Esc on the form drops it for what was shown before it. The
//!   benchmark the form starts shows once the session first reports it.
//! - `L` on a benchmark shows the LOG of one of its runs here. `d` brings
//!   PROJECTS to the front with the diff in the chat of the agent the
//!   benchmark was run from, as does the session putting on screen an
//!   agent BENCH does not show.
//! - A benchmark or the form puts the agent's chat tabs away, and the
//!   agent coming back gets them back, as going between apps does.

mod form;
mod keys;
mod list;

use form::BenchForm;
pub use form::{
    FormField, WorkChoice, AUTO_CLASSIFIER, BY_DIFFICULTY, DEFAULT_AGAINST, DIFFICULTIES,
};
pub use list::{BenchAgent, BenchRow};

use crate::app::{AgentsTab, App, NotificationType, PanelFocus};
use crate::events::{BenchExample, BenchRequest, BenchSession, BenchStatus, BenchView, UiEvent};
use ratatui::layout::Rect;
use std::collections::HashSet;
#[cfg(test)]
use {
    crate::app::ChatTab,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
};

/// What BENCH shows in the chat panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BenchMode {
    /// The benchmark with this id
    Detail(String),
    /// The form that sets a new one up
    Form,
    /// An agent that works for a benchmark, with its chat and tabs
    Agent {
        /// Id of the benchmark
        bench: String,
        /// Hex id of the agent
        agent: String,
    },
}

/// What BENCH knows about benchmarks, and where it is in them.
#[derive(Debug, Clone, Default)]
pub struct BenchState {
    /// Whether the list asked for has arrived
    loaded: bool,
    /// Every benchmark, of every tree, newest first
    pub benches: Vec<BenchView>,
    /// Sessions of the agent the form benchmarks from, to replay
    pub sessions: Vec<BenchSession>,
    /// The built-in example tasks, in the order a benchmark takes them
    pub examples: Vec<BenchExample>,
    /// What the chat panel shows in BENCH, kept while another app is in
    /// front; `None` for nothing
    pub mode: Option<BenchMode>,
    /// The list's highlighted row, by what Enter on it shows; `None` for
    /// the first row
    highlight: Option<BenchMode>,
    /// Benchmarks unfolded over their agents in the list, by id
    unfolded: HashSet<String>,
    /// First visible row of the list
    list_scroll: usize,
    /// Where the list's rows were drawn this frame
    rows_area: Option<Rect>,
    /// Where "+ New bench" was drawn this frame
    new_bench_area: Option<Rect>,
    /// What was shown when the form opened, shown again when the form is
    /// dropped
    before_form: Option<BenchMode>,
    /// Whether the form asked to start its benchmark and the session has
    /// not answered yet
    start_unanswered: bool,
    /// Whether the next new benchmark heard is the one the form started,
    /// to show in the form's place
    show_started: bool,
    /// Run picked in the detail view, for diff and apply
    pub run: Option<usize>,
    /// Lines scrolled down in the detail view
    pub scroll: u16,
    pub form: BenchForm,
    /// Where the form was drawn in the last frame, and the screen row of
    /// each of its rows, for clicks; empty when the form is not shown
    form_area: Option<Rect>,
    form_rows: Vec<(u16, FormField)>,
}

impl BenchState {
    #[must_use]
    pub const fn loaded(&self) -> bool {
        self.loaded
    }

    fn find(&self, id: &str) -> Option<&BenchView> {
        self.benches.iter().find(|b| b.id == id)
    }

    /// The benchmark the detail view shows.
    #[must_use]
    pub fn shown(&self) -> Option<&BenchView> {
        match &self.mode {
            Some(BenchMode::Detail(id)) => self.find(id),
            _ => None,
        }
    }

    /// The agent shown with its chat, which works for a benchmark.
    #[must_use]
    pub(in crate::app) fn shown_agent(&self) -> Option<&str> {
        match &self.mode {
            Some(BenchMode::Agent { agent, .. }) => Some(agent),
            _ => None,
        }
    }

    /// Whether the chat panel has something to show: the form, a listed
    /// benchmark, or an agent's chat.
    pub(super) fn showing(&self) -> bool {
        matches!(self.mode, Some(BenchMode::Form | BenchMode::Agent { .. }))
            || self.shown().is_some()
    }

    fn forget_areas(&mut self) {
        self.rows_area = None;
        self.new_bench_area = None;
        self.form_area = None;
        self.form_rows.clear();
    }

    fn list(&mut self, mut benches: Vec<BenchView>) {
        benches.reverse();
        // The list is built from records, which know nothing of the runs
        // under way: keep those last heard until the next report.
        for view in &mut benches {
            if view.status == BenchStatus::Running && view.live.is_empty() {
                if let Some(known) = self.benches.iter_mut().find(|b| b.id == view.id) {
                    view.live = std::mem::take(&mut known.live);
                }
            }
        }
        self.benches = benches;
        self.loaded = true;
        if let Some(BenchMode::Detail(id)) = &self.mode {
            if self.find(id).is_none() {
                self.mode = None;
            }
        }
    }

    /// Put `view` in place of the benchmark with its id, else first, as
    /// the newest. Returns whether it is new.
    fn upsert(&mut self, view: BenchView) -> bool {
        if let Some(known) = self.benches.iter_mut().find(|b| b.id == view.id) {
            *known = view;
            return false;
        }
        self.benches.insert(0, view);
        true
    }

    /// Scroll the detail view `lines` down, or up for a negative count.
    pub(in crate::app) fn scroll_by(&mut self, lines: isize) {
        if !matches!(self.mode, Some(BenchMode::Detail(_))) {
            return;
        }
        let by = u16::try_from(lines.unsigned_abs()).unwrap_or(u16::MAX);
        self.scroll = if lines < 0 {
            self.scroll.saturating_sub(by)
        } else {
            self.scroll.saturating_add(by)
        };
    }
}

/// The first line of `text` with words on it, trimmed.
fn first_line(text: &str) -> String {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim()
        .to_string()
}

impl App {
    /// BENCH's state.
    #[must_use]
    pub const fn bench(&self) -> &BenchState {
        &self.bench
    }

    /// Whether BENCH is the app in front.
    #[must_use]
    pub(crate) const fn bench_app(&self) -> bool {
        matches!(self.agents_tab, AgentsTab::Bench)
    }

    /// Whether BENCH is in front and draws the chat panel itself: a
    /// benchmark, the form, or what it says with neither. Not while it
    /// shows an agent's chat.
    #[must_use]
    pub(crate) fn bench_draws_center(&self) -> bool {
        self.bench_app() && self.bench.shown_agent().is_none()
    }

    /// Keep the detail view's scroll at most `top`, the line the last
    /// frame showed on top, so scrolling back up starts moving at once.
    pub(crate) fn clamp_bench_scroll(&mut self, top: u16) {
        if matches!(self.bench.mode, Some(BenchMode::Detail(_))) {
            self.bench.scroll = self.bench.scroll.min(top);
        }
    }

    #[cfg(test)]
    pub(crate) fn bench_mut(&mut self) -> &mut BenchState {
        &mut self.bench
    }

    /// Send `request` naming `agent_id`: the agent the form benchmarks
    /// from, whose sessions it offers, or the one whose chat shows a
    /// diff. The session takes any other request alike from every agent.
    fn send_bench_for(&self, agent_id: &str, request: BenchRequest) {
        self.try_send_event(UiEvent::Bench {
            agent_id: agent_id.to_string(),
            request,
        });
    }

    /// Send `request`, which is about no agent in particular.
    fn send_bench(&self, request: BenchRequest) {
        self.send_bench_for(&self.active_agent_id, request);
    }

    /// The user came to BENCH: ask for every benchmark.
    pub(in crate::app) fn enter_bench_app(&mut self) {
        self.send_bench(BenchRequest::List);
    }

    /// The user left BENCH, which keeps what it shows for coming back.
    pub(in crate::app) fn leave_bench_app(&mut self) {
        self.bench.forget_areas();
    }

    /// What BENCH shows on coming to it: what it was left on, else the
    /// highlighted benchmark, else the newest; `None` with none listed.
    /// An agent left on that is neither listed nor archived any more
    /// gives way to its benchmark.
    pub(in crate::app) fn bench_to_show(&self) -> Option<BenchMode> {
        match &self.bench.mode {
            Some(BenchMode::Agent { agent, .. })
                if self.is_listed(agent) || self.is_archived_agent(agent) =>
            {
                return self.bench.mode.clone();
            }
            Some(BenchMode::Agent { bench, .. }) if self.bench.find(bench).is_some() => {
                return Some(BenchMode::Detail(bench.clone()));
            }
            Some(BenchMode::Agent { .. }) => {}
            _ if self.bench.showing() => return self.bench.mode.clone(),
            _ => {}
        }
        self.highlighted_bench()
            .or_else(|| self.bench.benches.first().map(|view| view.id.as_str()))
            .map(|id| BenchMode::Detail(id.to_string()))
    }

    /// Coming to BENCH: show [`Self::bench_to_show`], or nothing.
    pub(in crate::app) fn show_bench_app(&mut self) {
        match self.bench_to_show() {
            Some(mode) => self.show_bench(mode),
            None if self.bench.shown_agent().is_some() => {
                self.bench.mode = None;
                self.put_chat_tabs_away();
            }
            None => {}
        }
    }

    /// Show `mode` in the chat panel and highlight its row: a listed
    /// benchmark, from its top with no run picked unless it was shown
    /// already; the form; or an agent of a benchmark, which unfolds it.
    pub(in crate::app) fn show_bench(&mut self, mode: BenchMode) {
        match &mode {
            BenchMode::Detail(id) if self.bench.find(id).is_none() => return,
            BenchMode::Agent { bench, .. } => {
                self.bench.unfolded.insert(bench.clone());
            }
            _ => {}
        }
        self.bench.highlight = Some(mode.clone());
        let agent_was_shown = self.bench.shown_agent().is_some();
        if self.bench.mode.as_ref() != Some(&mode) {
            match mode {
                BenchMode::Form => self.open_bench_form(),
                BenchMode::Detail(_) => {
                    self.bench.scroll = 0;
                    self.bench.run = None;
                    self.bench.mode = Some(mode);
                }
                BenchMode::Agent { .. } => self.bench.mode = Some(mode),
            }
        }
        if self.bench_app() {
            self.settle_bench_chat(agent_was_shown);
        }
    }

    /// Bring the chat panel in line with what BENCH shows, which showed
    /// an agent before if `agent_was_shown`. An agent comes on screen
    /// with its tabs, given back as it had them when they were put away;
    /// a benchmark or the form puts them away.
    fn settle_bench_chat(&mut self, agent_was_shown: bool) {
        match self.bench.shown_agent().map(str::to_string) {
            Some(agent) => {
                self.switch_on_screen(&agent);
                if !agent_was_shown {
                    self.give_chat_tabs_back();
                }
            }
            None if agent_was_shown => self.put_chat_tabs_away(),
            None => {}
        }
    }

    /// While BENCH is in front showing nothing, show the highlighted
    /// benchmark, else the newest.
    fn show_bench_if_blank(&mut self) {
        if self.bench_app() && !self.bench.showing() {
            if let Some(mode) = self.bench_to_show() {
                self.show_bench(mode);
            }
        }
    }

    /// Drop the form for what was shown before it, a benchmark if it is
    /// still listed or an agent, and hand the keys back to the list.
    fn drop_bench_form(&mut self) {
        self.bench.mode = None;
        if let Some(mode) = self.bench.before_form.take() {
            self.show_bench(mode);
        }
        self.focus_bench_list();
    }

    /// Hand the keys to the list, when the agents panel is on screen.
    fn focus_bench_list(&mut self) {
        if self.swarm_panel_visible() {
            self.focus = PanelFocus::Swarm;
        }
    }

    /// The chat panel's title in BENCH, and what it says beside it: the
    /// benchmark shown and what it is about, or the form's. `None` with
    /// nothing shown, and for an agent, whose chat has its tabs.
    #[must_use]
    pub(in crate::app) fn bench_chat_title(&self) -> Option<(String, String)> {
        match &self.bench.mode {
            Some(BenchMode::Form) => Some(("New benchmark".to_string(), String::new())),
            Some(BenchMode::Detail(_)) => self
                .bench
                .shown()
                .map(|view| (view.name.clone(), first_line(&view.title))),
            Some(BenchMode::Agent { .. }) | None => None,
        }
    }

    /// What the chat says while BENCH shows nothing.
    pub(in crate::app) fn bench_empty_message(&self) -> &'static str {
        if !self.bench.loaded {
            "Loading the benchmarks…"
        } else if self.bench.benches.is_empty() {
            "No benchmarks yet. + New bench on the left runs the same work (your last \
             prompt, a new one, example coding tasks or an earlier session) on two arms, \
             Gate or a model each, and compares what they cost and how well they did."
        } else {
            "Pick a benchmark on the left to see its numbers, or + New bench to run one."
        }
    }

    /// What the empty input says in BENCH: what the form's row takes,
    /// when it takes text, else that only commands run.
    #[must_use]
    pub(in crate::app) fn bench_placeholder(&self) -> String {
        let field = self.bench.form.field;
        if self.bench.mode != Some(BenchMode::Form) || !field.is_text() {
            return "BENCH has no chat; / runs a command".to_string();
        }
        let what = match field {
            FormField::Prompt => "the prompt",
            FormField::Check => "the check command",
            FormField::Budget => "the budget in USD",
            _ => "part of a model's name",
        };
        format!("Type {what}, then Enter")
    }

    /// Whether a running benchmark is on screen, in BENCH or the progress
    /// panel, whose status and working runs shimmer frame by frame.
    #[must_use]
    pub fn bench_animating(&self) -> bool {
        let running = |view: &BenchView| view.status == BenchStatus::Running;
        (self.bench_app() && self.bench.benches.iter().any(running))
            || self.bench_panel_shown().is_some_and(running)
    }

    pub(in crate::app) fn cmd_bench_list(&mut self, benches: Vec<BenchView>) {
        for view in &benches {
            self.bench_panel.heard(view);
        }
        self.bench.list(benches);
        self.show_bench_if_blank();
    }

    pub(in crate::app) fn cmd_bench_upsert(&mut self, view: BenchView) {
        self.bench_panel.heard(&view);
        let id = view.id.clone();
        let new = self.bench.upsert(view);
        if new
            && std::mem::take(&mut self.bench.show_started)
            && self.bench.mode == Some(BenchMode::Form)
        {
            self.bench.unfolded.insert(id.clone());
            self.show_bench(BenchMode::Detail(id));
        }
        self.show_bench_if_blank();
    }

    pub(in crate::app) fn cmd_bench_notice(&mut self, notice: Result<String, String>) {
        if std::mem::take(&mut self.bench.start_unanswered) {
            if notice.is_ok() {
                // A fresh form for the next benchmark.
                self.bench.reset_form();
            } else {
                self.bench.show_started = false;
            }
        }
        self.notifications.current = Some(match notice {
            Ok(text) => (text, NotificationType::Success),
            Err(problem) => (problem, NotificationType::Warning),
        });
    }
}

#[cfg(test)]
mod tests;
