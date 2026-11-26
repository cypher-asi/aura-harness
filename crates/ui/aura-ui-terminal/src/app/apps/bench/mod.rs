//! BENCH, an app of its own beside the agents: benchmarks that run the
//! same work on two arms (Gate, a pinned model, or a model per
//! difficulty) and compare what they cost and how well they did. The
//! event loop runs them as `/compare` comparisons.
//!
//! The agents panel's BENCH tab lists every benchmark, of every tree,
//! newest first, then "+ New bench" (index `benches.len()`), which the
//! panel pins to its bottom row. The chat panel shows the benchmark
//! picked there, or the form that sets a new one up (see `form`), under
//! a header with no tabs. No agent is on screen: the right column, the
//! queue and the progress panel stay away, and the input runs commands
//! and fills the form's text rows but sends no message.
//!
//! - Enter or a click on a row shows its benchmark, or the form. Enter
//!   also hands the keys to it, and Esc hands them back to the list.
//! - Coming to BENCH shows what it was left on, else the highlighted
//!   benchmark, else the newest. So does a list arriving while it shows
//!   nothing.
//! - Esc on the form drops it for the benchmark shown before it. The
//!   benchmark the form starts shows once the session first reports it.
//! - What needs an agent's chat brings PROJECTS to the front: `L` with a
//!   run's log, `d` with the diff in the chat of the agent the benchmark
//!   was run from. So does the session putting an agent on screen.

mod form;

use form::BenchForm;
pub use form::{
    FormField, WorkChoice, AUTO_CLASSIFIER, BY_DIFFICULTY, DEFAULT_AGAINST, DIFFICULTIES,
};

use crate::app::{
    contains, follow_selection, AgentsTab, App, ChatTab, NotificationType, PanelFocus,
};
use crate::events::{BenchExample, BenchRequest, BenchSession, BenchStatus, BenchView, UiEvent};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

/// Rows a PageUp or PageDown moves the highlight by.
const PAGE_STEP: usize = 5;

/// What BENCH shows in the chat panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BenchMode {
    /// The benchmark with this id
    Detail(String),
    /// The form that sets a new one up
    Form,
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
    /// Highlighted row of the list; `benches.len()` is "+ New bench"
    selected: usize,
    /// First visible row of the list
    list_scroll: usize,
    /// Where the list's rows were drawn this frame
    rows_area: Option<Rect>,
    /// Where "+ New bench" was drawn this frame
    new_bench_area: Option<Rect>,
    /// The benchmark shown when the form opened, shown again when the
    /// form is dropped
    before_form: Option<String>,
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

    /// Whether the chat panel has something to show: the form, or a
    /// listed benchmark.
    pub(super) fn showing(&self) -> bool {
        self.mode == Some(BenchMode::Form) || self.shown().is_some()
    }

    /// The highlighted benchmark; `None` on "+ New bench".
    fn highlighted(&self) -> Option<&BenchView> {
        self.benches.get(self.selected)
    }

    fn set_selected(&mut self, row: usize) {
        self.selected = row.min(self.benches.len());
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
        let highlighted = self.highlighted().map(|view| view.id.clone());
        self.benches = benches;
        self.loaded = true;
        // The highlight stays on its benchmark, or on "+ New bench".
        let row = match highlighted {
            Some(id) => self
                .benches
                .iter()
                .position(|b| b.id == id)
                .unwrap_or(self.selected),
            None => usize::MAX,
        };
        self.set_selected(row);
        if let Some(BenchMode::Detail(id)) = &self.mode {
            if self.find(id).is_none() {
                self.mode = None;
            }
        }
    }

    /// Put `view` in place of the benchmark with its id, else first, as
    /// the newest; the highlight stays on its row. Returns whether it is
    /// new.
    fn upsert(&mut self, view: BenchView) -> bool {
        if let Some(known) = self.benches.iter_mut().find(|b| b.id == view.id) {
            *known = view;
            return false;
        }
        self.benches.insert(0, view);
        self.selected += 1;
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
    pub(in crate::app) fn bench_to_show(&self) -> Option<BenchMode> {
        if self.bench.showing() {
            return self.bench.mode.clone();
        }
        self.bench
            .highlighted()
            .or_else(|| self.bench.benches.first())
            .map(|view| BenchMode::Detail(view.id.clone()))
    }

    /// Show `mode` in the chat panel and highlight its row: a listed
    /// benchmark, from its top with no run picked unless it was shown
    /// already, or the form.
    pub(in crate::app) fn show_bench(&mut self, mode: BenchMode) {
        let row = match &mode {
            BenchMode::Detail(id) => match self.bench.benches.iter().position(|b| &b.id == id) {
                Some(row) => row,
                None => return,
            },
            BenchMode::Form => self.bench.benches.len(),
        };
        if self.bench.mode.as_ref() != Some(&mode) {
            if mode == BenchMode::Form {
                self.open_bench_form();
            } else {
                self.bench.scroll = 0;
                self.bench.run = None;
                self.bench.mode = Some(mode);
            }
        }
        self.bench.set_selected(row);
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

    /// Drop the form for the benchmark shown before it, if it is still
    /// listed, and hand the keys back to the list.
    fn drop_bench_form(&mut self) {
        self.bench.mode = None;
        if let Some(id) = self.bench.before_form.take() {
            self.show_bench(BenchMode::Detail(id));
        }
        self.focus_bench_list();
    }

    /// Hand the keys to the list, when the agents panel is on screen.
    fn focus_bench_list(&mut self) {
        if self.swarm_panel_visible() {
            self.focus = PanelFocus::Swarm;
        }
    }

    /// Row of the list for what the chat panel shows: its benchmark's,
    /// or "+ New bench" for the form.
    #[must_use]
    pub(crate) fn shown_bench_row(&self) -> Option<usize> {
        match &self.bench.mode {
            Some(BenchMode::Form) => Some(self.bench.benches.len()),
            Some(BenchMode::Detail(id)) => self.bench.benches.iter().position(|b| &b.id == id),
            None => None,
        }
    }

    /// Highlighted row of the list, where `bench().benches.len()` is
    /// "+ New bench".
    #[must_use]
    pub(crate) const fn selected_bench(&self) -> usize {
        self.bench.selected
    }

    /// First visible row of the list.
    #[must_use]
    pub(crate) const fn bench_list_scroll(&self) -> usize {
        self.bench.list_scroll
    }

    /// Keep the list's highlight inside rows `visible_rows` tall.
    /// "+ New bench" is pinned below them, so it never scrolls.
    pub(crate) fn sync_bench_list_scroll(&mut self, visible_rows: usize) {
        let state = &mut self.bench;
        state.list_scroll = follow_selection(
            state.list_scroll,
            state.selected,
            visible_rows,
            state.benches.len(),
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

    /// Row of the list drawn at a screen cell: a benchmark, or
    /// "+ New bench" (`bench().benches.len()`).
    pub(in crate::app) fn bench_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let state = &self.bench;
        if contains(state.new_bench_area, column, row) {
            return Some(state.benches.len());
        }
        let rows = state
            .rows_area
            .filter(|&rows| contains(Some(rows), column, row))?;
        let index = state.list_scroll + usize::from(row - rows.y);
        (index < state.benches.len()).then_some(index)
    }

    /// Clicking a row highlights it and shows it, as Enter does; only the
    /// form takes the keys from the list.
    pub(in crate::app) fn click_bench_row(&mut self, column: u16, row: u16) {
        let Some(index) = self.bench_row_at(column, row) else {
            return;
        };
        self.bench.set_selected(index);
        self.activate_bench_row();
    }

    /// Move the highlight `delta` rows, as the wheel does.
    pub(in crate::app) fn move_bench_highlight(&mut self, delta: isize) {
        let row = self.bench.selected.saturating_add_signed(delta);
        self.bench.set_selected(row);
    }

    /// Keys of the BENCH list. Returns whether the key was used.
    pub(in crate::app) fn handle_bench_list_key(&mut self, key: KeyEvent) -> bool {
        let current = self.bench.selected;
        match key.code {
            KeyCode::Up => self.bench.set_selected(current.saturating_sub(1)),
            KeyCode::Down => self.bench.set_selected(current + 1),
            KeyCode::PageUp => self.bench.set_selected(current.saturating_sub(PAGE_STEP)),
            KeyCode::PageDown => self.bench.set_selected(current + PAGE_STEP),
            KeyCode::Home => self.bench.set_selected(0),
            KeyCode::End => self.bench.set_selected(usize::MAX),
            KeyCode::Enter => {
                self.activate_bench_row();
                self.focus = PanelFocus::Chat;
            }
            KeyCode::Char('n') => {
                self.bench.set_selected(usize::MAX);
                self.activate_bench_row();
            }
            KeyCode::Char('r') => self.send_bench(BenchRequest::List),
            KeyCode::Char('c') => self.send_bench(BenchRequest::Cancel),
            KeyCode::Left | KeyCode::Right => self.shift_agents_tab(key.code),
            _ => return false,
        }
        true
    }

    /// Show the highlighted row: its benchmark, or the form for
    /// "+ New bench", which takes the keys.
    fn activate_bench_row(&mut self) {
        match self.bench.highlighted().map(|view| view.id.clone()) {
            Some(id) => self.show_bench(BenchMode::Detail(id)),
            None => {
                self.show_bench(BenchMode::Form);
                self.focus = PanelFocus::Chat;
            }
        }
    }

    /// The chat panel's title in BENCH, and what it says beside it: the
    /// benchmark shown and what it is about, or the form's. `None` with
    /// nothing shown.
    #[must_use]
    pub(in crate::app) fn bench_chat_title(&self) -> Option<(String, String)> {
        match &self.bench.mode {
            Some(BenchMode::Form) => Some(("New benchmark".to_string(), String::new())),
            Some(BenchMode::Detail(_)) => self
                .bench
                .shown()
                .map(|view| (view.name.clone(), first_line(&view.title))),
            None => None,
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
        self.notification = Some(match notice {
            Ok(text) => (text, NotificationType::Success),
            Err(problem) => (problem, NotificationType::Warning),
        });
    }

    /// A key in BENCH's chat panel with nothing typed: the benchmark's,
    /// or the form's. Returns whether it used the key.
    pub(in crate::app) fn handle_bench_key(&mut self, key: KeyEvent) -> bool {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return false;
        }
        match self.bench.mode.clone() {
            Some(BenchMode::Detail(id)) => self.bench_detail_key(key, &id),
            Some(BenchMode::Form) => self.bench_form_key(key),
            None if key.code == KeyCode::Esc => {
                self.focus_bench_list();
                true
            }
            None => false,
        }
    }

    fn bench_detail_key(&mut self, key: KeyEvent, id: &str) -> bool {
        let Some(view) = self.bench.shown() else {
            return false;
        };
        let runs = view.runs.len();
        let has_copies = view.status.has_copies() && view.status != BenchStatus::Running;
        let examples = !view.tasks.is_empty();
        let root = view.root.clone();
        let run = self
            .bench
            .run
            .and_then(|i| view.runs.get(i))
            .map(|r| r.name.clone());
        // The picked run, else the first one under way.
        let log_of = match self.bench.run.and_then(|i| view.runs.get(i)) {
            Some(picked) => Some(picked.agent_id.clone()).filter(|id| !id.is_empty()),
            None => view.live.iter().find_map(|live| live.agent_id.clone()),
        };
        let id = id.to_string();
        match key.code {
            KeyCode::Esc | KeyCode::Backspace => self.focus_bench_list(),
            KeyCode::Up | KeyCode::Char('k') => self.bench.scroll_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.bench.scroll_by(1),
            KeyCode::PageUp => self.bench.scroll_by(-10),
            KeyCode::PageDown => self.bench.scroll_by(10),
            KeyCode::Home | KeyCode::Char('g') => self.bench.scroll = 0,
            KeyCode::Left | KeyCode::Char('h') if runs > 0 => {
                self.bench.run = Some(self.bench.run.map_or(runs - 1, |i| i.saturating_sub(1)));
            }
            KeyCode::Right | KeyCode::Char('l') if runs > 0 => {
                self.bench.run = Some(self.bench.run.map_or(0, |i| (i + 1).min(runs - 1)));
            }
            KeyCode::Char('d') if has_copies => self.show_bench_diff(&root, id, run),
            KeyCode::Char('y') if has_copies && examples => {
                self.notification = Some((
                    "Example tasks ran in their own fixtures; there is nothing to apply"
                        .to_string(),
                    NotificationType::Warning,
                ));
            }
            KeyCode::Char('y') if has_copies => match run {
                Some(run) => self.send_bench(BenchRequest::Apply { id, run }),
                None => {
                    self.notification = Some((
                        "Pick a run with ←/→ first, then y applies it".to_string(),
                        NotificationType::Warning,
                    ));
                }
            },
            KeyCode::Char('x') if has_copies => self.send_bench(BenchRequest::Dismiss { id }),
            KeyCode::Char('c') => self.send_bench(BenchRequest::Cancel),
            KeyCode::Char('L') => match log_of {
                Some(agent) => {
                    self.switch_app(AgentsTab::Projects);
                    self.open_log_of(agent);
                }
                None => {
                    self.notification = Some((
                        "This run has no log; pick a run with ←/→ first".to_string(),
                        NotificationType::Warning,
                    ));
                }
            },
            _ => return false,
        }
        true
    }

    /// Show the diff of benchmark `id`'s run `run`, or of every run, in
    /// the chat of `root`, the agent it was run from: PROJECTS comes to
    /// the front with that agent on CHAT.
    fn show_bench_diff(&mut self, root: &str, id: String, run: Option<String>) {
        self.switch_app(AgentsTab::Projects);
        if root.is_empty() || root == self.active_agent_id {
            self.show_chat_tab(ChatTab::Chat);
        } else {
            self.give_chat_tab_back(root, ChatTab::Chat);
            self.request_agent_switch(root.to_string());
        }
        self.send_bench_for(root, BenchRequest::Diff { id, run });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{AgentSummary, BenchArm, BenchRun, UiCommand};
    use tokio::sync::mpsc;

    fn view(id: &str) -> BenchView {
        BenchView {
            id: id.to_string(),
            root: "root".to_string(),
            name: format!("Compare {id}"),
            title: format!("work {id}\nmore of it"),
            arms: [BenchArm::default(), BenchArm::default()],
            ..BenchView::default()
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    /// Top-level agent `root` with `kid` under it, `root` on screen, and
    /// benchmarks `1` and `2` listed, `2` the newest; and what the app
    /// sends.
    fn app_with_benches() -> (App, mpsc::Receiver<UiEvent>) {
        let (tx, rx) = mpsc::channel(64);
        let mut app = App::new().with_event_sender(tx);
        let agent = |id: &str, depth| AgentSummary {
            id: id.to_string(),
            name: id.to_string(),
            depth,
            ..AgentSummary::default()
        };
        app.process_command(UiCommand::SetAgents(vec![
            agent("root", 0),
            agent("kid", 1),
        ]));
        app.process_command(UiCommand::SetActiveAgent("root".to_string()));
        app.process_command(UiCommand::BenchList {
            benches: vec![view("1"), view("2")],
        });
        (app, rx)
    }

    /// The benchmark requests sent since the last call, as
    /// `<agent> <request>`.
    fn bench_requests(rx: &mut mpsc::Receiver<UiEvent>) -> Vec<String> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|event| match event {
                UiEvent::Bench { agent_id, request } => Some(format!("{agent_id} {request:?}")),
                _ => None,
            })
            .collect()
    }

    fn detail(id: &str) -> BenchMode {
        BenchMode::Detail(id.to_string())
    }

    #[test]
    fn the_list_is_newest_first_and_upserts_replace_in_place() {
        let mut state = BenchState::default();
        state.list(vec![view("1"), view("2")]);
        assert_eq!(state.benches[0].id, "2");
        assert!(state.loaded());
        let mut changed = view("1");
        changed.title = "changed".to_string();
        assert!(!state.upsert(changed));
        assert_eq!(state.benches.len(), 2);
        assert_eq!(state.benches[1].title, "changed");
        assert!(state.upsert(view("3")));
        assert_eq!(state.benches[0].id, "3");
    }

    #[test]
    fn every_tree_s_benchmarks_are_listed_whoever_is_on_screen() {
        let mut app = App::new();
        app.active_agent_id = "user".to_string();
        let mut elsewhere = view("2");
        elsewhere.root = "other".to_string();
        app.cmd_bench_list(vec![view("1"), elsewhere]);
        assert_eq!(app.bench.benches.len(), 2);
        app.bench.mode = Some(detail("2"));

        app.active_agent_id = "other".to_string();
        let mut third = view("3");
        third.root = "third".to_string();
        app.cmd_bench_upsert(third);
        assert_eq!(app.bench.benches[0].id, "3", "any tree's new benchmark");
        assert_eq!(app.bench.benches.len(), 3);

        app.cmd_bench_list(vec![view("1")]);
        assert_eq!(app.bench.mode, None, "the benchmark shown is gone");
    }

    #[test]
    fn bench_comes_after_agents_and_puts_no_agent_on_screen() {
        let (mut app, mut rx) = app_with_benches();
        app.show_chat_tab(ChatTab::Graph);
        bench_requests(&mut rx);

        app.handle_key(KeyEvent::new(KeyCode::Char('4'), KeyModifiers::ALT));
        assert_eq!(app.agents_tab(), AgentsTab::Bench);
        assert_eq!(bench_requests(&mut rx), ["root List"]);
        assert_eq!(app.bench.mode, Some(detail("2")), "the newest");
        assert_eq!(app.selected_bench(), 0);
        assert_eq!(
            app.app_chat_title(),
            Some(("Compare 2".to_string(), "work 2".to_string()))
        );
        assert_eq!(app.chat_tab(), ChatTab::Chat, "GRAPH is put away");
        assert!(
            app.chat_tab_layout(80).cells.is_empty(),
            "the bar has no tabs"
        );
        app.show_chat_tab(ChatTab::Log);
        assert_eq!(app.chat_tab(), ChatTab::Chat, "nor can one be shown");
        assert!(app.no_agent_on_screen());
        assert!(!app.record_panel_visible());
        assert!(!app.sandbox_panel_visible());

        app.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::ALT));
        assert_eq!(app.agents_tab(), AgentsTab::Projects);
        assert_eq!(app.chat_tab(), ChatTab::Graph, "root has its tab back");
        assert!(!app.no_agent_on_screen());
    }

    #[test]
    fn going_between_channels_and_bench_keeps_the_agent_s_tab() {
        let (mut app, _rx) = app_with_benches();
        app.show_chat_tab(ChatTab::Graph);
        app.show_agents_tab(AgentsTab::Channels);
        app.show_agents_tab(AgentsTab::Bench);
        app.show_agents_tab(AgentsTab::Projects);
        assert_eq!(app.chat_tab(), ChatTab::Graph);
    }

    #[test]
    fn enter_shows_a_benchmark_and_bench_comes_back_on_it() {
        let (mut app, _rx) = app_with_benches();
        app.focus = PanelFocus::Swarm;
        app.show_agents_tab(AgentsTab::Bench);
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.selected_bench(), 1);
        assert_eq!(
            app.bench.mode,
            Some(detail("2")),
            "the highlight alone shows nothing"
        );

        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.bench.mode, Some(detail("1")));
        assert_eq!(app.focus(), PanelFocus::Chat, "its keys come first");
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.bench.scroll, 1);
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.focus(), PanelFocus::Swarm, "the list has them back");
        assert_eq!(app.bench.mode, Some(detail("1")), "still shown");

        app.show_agents_tab(AgentsTab::Projects);
        app.show_agents_tab(AgentsTab::Bench);
        assert_eq!(app.bench.mode, Some(detail("1")), "what BENCH was left on");
        assert_eq!(app.bench.scroll, 1, "where it was left");

        // A list without it shows the newest in its place.
        app.process_command(UiCommand::BenchList {
            benches: vec![view("2")],
        });
        assert_eq!(app.bench.mode, Some(detail("2")));
        assert_eq!(app.bench.scroll, 0);
    }

    #[test]
    fn a_click_shows_a_benchmark_and_keeps_the_keys_on_the_list() {
        let (mut app, _rx) = app_with_benches();
        app.show_agents_tab(AgentsTab::Bench);
        app.focus = PanelFocus::Swarm;
        app.set_bench_rows_area(Rect::new(1, 2, 30, 5));
        app.set_new_bench_area(Rect::new(1, 7, 30, 1));
        assert_eq!(app.bench_row_at(5, 3), Some(1));
        assert_eq!(app.bench_row_at(5, 4), None, "below the rows");
        assert_eq!(app.bench_row_at(5, 7), Some(2), "+ New bench");

        app.click_bench_row(5, 3);
        assert_eq!(app.bench.mode, Some(detail("1")));
        assert_eq!(app.focus(), PanelFocus::Swarm);
        app.click_bench_row(5, 7);
        assert_eq!(app.bench.mode, Some(BenchMode::Form));
        assert_eq!(app.focus(), PanelFocus::Chat, "the form takes the keys");
    }

    #[test]
    fn the_form_opens_from_the_list_and_esc_shows_the_benchmark_before_it() {
        let (mut app, mut rx) = app_with_benches();
        app.focus = PanelFocus::Swarm;
        app.show_agents_tab(AgentsTab::Bench);
        bench_requests(&mut rx);

        app.handle_key(key(KeyCode::Char('n')));
        assert_eq!(app.bench.mode, Some(BenchMode::Form));
        assert_eq!(app.selected_bench(), 2, "+ New bench");
        assert_eq!(app.shown_bench_row(), Some(2));
        assert_eq!(app.focus(), PanelFocus::Chat);
        assert_eq!(bench_requests(&mut rx), ["root Sessions"]);
        assert_eq!(
            app.app_chat_title(),
            Some(("New benchmark".to_string(), String::new()))
        );

        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.bench.mode, Some(detail("2")));
        assert_eq!(app.selected_bench(), 0);
        assert_eq!(app.focus(), PanelFocus::Swarm);
    }

    #[test]
    fn a_benchmark_the_form_starts_shows_once_the_session_reports_it() {
        let (mut app, mut rx) = app_with_benches();
        app.show_agents_tab(AgentsTab::Bench);
        app.show_bench(BenchMode::Form);
        app.focus = PanelFocus::Chat;
        app.bench.form.estimate = Some(Ok("about $1".to_string()));
        app.bench.form.field = FormField::Go;
        bench_requests(&mut rx);

        app.handle_key(key(KeyCode::Enter));
        assert_eq!(bench_requests(&mut rx), ["root Start"]);
        // The session's answer can come before the benchmark's first
        // report.
        app.process_command(UiCommand::BenchNotice(Ok("Compare 3 started".to_string())));
        assert_eq!(app.bench.mode, Some(BenchMode::Form));
        assert!(!app.bench.form.ready(), "a fresh form");
        app.process_command(UiCommand::BenchUpsert(Box::new(view("3"))));
        assert_eq!(app.bench.mode, Some(detail("3")));
        assert_eq!(app.selected_bench(), 0);

        // Benchmarks started elsewhere are only listed.
        app.process_command(UiCommand::BenchUpsert(Box::new(view("4"))));
        assert_eq!(app.bench.mode, Some(detail("3")));
        assert_eq!(app.selected_bench(), 1, "still on it");
    }

    #[test]
    fn bench_sends_no_message_but_fills_the_form_and_runs_commands() {
        let (mut app, mut rx) = app_with_benches();
        app.show_agents_tab(AgentsTab::Bench);
        app.focus = PanelFocus::Chat;
        app.input = "hello".to_string();
        app.cursor_pos = 5;
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.input(), "hello", "the draft stays");
        let (text, _) = app.notification.clone().expect("a refusal");
        assert!(text.contains("BENCH has no chat"), "{text}");
        assert!(std::iter::from_fn(|| rx.try_recv().ok())
            .all(|event| !matches!(event, UiEvent::UserMessage { .. })));

        // From the list too, Enter puts the text on the form's row.
        app.show_bench(BenchMode::Form);
        app.bench.form.field = FormField::Prompt;
        app.focus = PanelFocus::Swarm;
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.bench.form.prompt, "hello");
        assert_eq!(app.input(), "");

        app.bench.form.field = FormField::Work;
        app.input = "/agents".to_string();
        app.cursor_pos = 7;
        app.handle_key(key(KeyCode::Enter));
        assert!(!app.swarm_panel_visible(), "commands run");
    }

    #[test]
    fn an_agent_coming_on_screen_brings_projects_back_on_its_tab() {
        let (mut app, _rx) = app_with_benches();
        app.process_command(UiCommand::SetActiveAgent("kid".to_string()));
        app.show_chat_tab(ChatTab::Log);
        app.process_command(UiCommand::SetActiveAgent("root".to_string()));
        app.show_chat_tab(ChatTab::Graph);
        app.show_agents_tab(AgentsTab::Bench);

        app.process_command(UiCommand::SetActiveAgent("root".to_string()));
        assert_eq!(app.agents_tab(), AgentsTab::Bench, "the same agent again");

        app.process_command(UiCommand::SetActiveAgent("kid".to_string()));
        assert_eq!(app.agents_tab(), AgentsTab::Projects);
        assert_eq!(app.chat_tab(), ChatTab::Log, "kid was left on LOG");
        app.process_command(UiCommand::SetActiveAgent("root".to_string()));
        assert_eq!(app.chat_tab(), ChatTab::Graph, "root was left on GRAPH");
    }

    #[test]
    fn a_run_s_log_and_a_diff_go_to_projects() {
        let (mut app, mut rx) = app_with_benches();
        let mut done = view("2");
        done.status = BenchStatus::Done;
        done.runs = vec![BenchRun {
            name: "a-1".to_string(),
            agent_id: "kid".to_string(),
            ..BenchRun::default()
        }];
        app.process_command(UiCommand::BenchUpsert(Box::new(done)));
        app.process_command(UiCommand::SetActiveAgent("kid".to_string()));
        app.show_chat_tab(ChatTab::Graph);
        app.show_agents_tab(AgentsTab::Bench);
        app.focus = PanelFocus::Chat;
        bench_requests(&mut rx);

        // The diff shows in the chat of the agent it was run from.
        app.handle_key(key(KeyCode::Char('d')));
        assert_eq!(app.agents_tab(), AgentsTab::Projects);
        assert_eq!(app.active_agent_id(), "root");
        assert_eq!(app.chat_tab(), ChatTab::Chat);
        assert_eq!(
            bench_requests(&mut rx),
            [r#"root Diff { id: "2", run: None }"#]
        );
        // The session confirms the switch.
        app.process_command(UiCommand::SetActiveAgent("root".to_string()));
        app.process_command(UiCommand::SetActiveAgent("kid".to_string()));
        assert_eq!(app.chat_tab(), ChatTab::Graph, "kid was left on GRAPH");

        // The log of the run picked, on the agent that ran it.
        app.process_command(UiCommand::SetActiveAgent("root".to_string()));
        app.show_agents_tab(AgentsTab::Bench);
        app.focus = PanelFocus::Chat;
        app.handle_key(key(KeyCode::Right));
        app.handle_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT));
        assert_eq!(app.agents_tab(), AgentsTab::Projects);
        assert_eq!(app.active_agent_id(), "kid");
        assert_eq!(app.chat_tab(), ChatTab::Log);
    }
}
