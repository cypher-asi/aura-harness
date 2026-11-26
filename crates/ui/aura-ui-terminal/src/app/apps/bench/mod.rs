//! The chat panel's BENCH tab: benchmarks that run the same work on two
//! arms (Gate, a pinned model, or a model per difficulty) and compare
//! what they cost and how well they did. The tab lists them, sets a new
//! one up with a form (see `form`), and shows one's numbers; the event
//! loop runs them as `/compare` comparisons.

mod form;

use form::BenchForm;
pub use form::{
    FormField, WorkChoice, AUTO_CLASSIFIER, BY_DIFFICULTY, DEFAULT_AGAINST, DIFFICULTIES,
};

use crate::app::{App, ChatTab, NotificationType};
use crate::events::{BenchExample, BenchRequest, BenchSession, BenchStatus, BenchView, UiEvent};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

/// What the BENCH tab shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BenchMode {
    /// Every benchmark, newest first
    #[default]
    List,
    /// The benchmark with this id
    Detail(String),
    /// The form that sets a new one up
    Form,
}

/// The BENCH tab's state.
#[derive(Debug, Clone, Default)]
pub struct BenchState {
    open: bool,
    /// Whether the list asked for has arrived
    loaded: bool,
    /// Hex id of the agent at the top of the tree whose benchmarks it lists
    root: Option<String>,
    /// Benchmarks of that tree, newest first
    pub benches: Vec<BenchView>,
    /// Sessions of the agent on screen, to replay
    pub sessions: Vec<BenchSession>,
    /// The built-in example tasks, in the order a benchmark takes them
    pub examples: Vec<BenchExample>,
    pub mode: BenchMode,
    /// Row of the list
    pub selected: usize,
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
    pub const fn is_open(&self) -> bool {
        self.open
    }

    pub(in crate::app) fn set_open(&mut self, open: bool) {
        self.open = open;
    }

    #[must_use]
    pub const fn loaded(&self) -> bool {
        self.loaded
    }

    /// The benchmark the detail view shows.
    #[must_use]
    pub fn shown(&self) -> Option<&BenchView> {
        match &self.mode {
            BenchMode::Detail(id) => self.benches.iter().find(|b| &b.id == id),
            _ => None,
        }
    }

    fn list(&mut self, root: String, mut benches: Vec<BenchView>) {
        if self.root.as_ref() != Some(&root) {
            self.selected = 0;
            self.run = None;
            self.scroll = 0;
        }
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
        self.root = Some(root);
        self.benches = benches;
        self.loaded = true;
        self.selected = self.selected.min(self.benches.len().saturating_sub(1));
        if let BenchMode::Detail(id) = &self.mode {
            if !self.benches.iter().any(|b| &b.id == id) {
                self.mode = BenchMode::List;
            }
        }
    }

    fn upsert(&mut self, view: BenchView) {
        if self.root.as_ref() != Some(&view.root) {
            return;
        }
        match self.benches.iter_mut().find(|b| b.id == view.id) {
            Some(known) => *known = view,
            None => {
                self.benches.insert(0, view);
                if self.mode == BenchMode::List {
                    self.selected = 0;
                }
            }
        }
    }

    fn move_selection(&mut self, down: bool) {
        let last = self.benches.len().saturating_sub(1);
        self.selected = if down {
            (self.selected + 1).min(last)
        } else {
            self.selected.saturating_sub(1)
        };
    }

    /// Scroll the detail view, or move along the list.
    pub(in crate::app) fn scroll_by(&mut self, lines: isize) {
        match self.mode {
            BenchMode::Detail(_) => {
                let by = u16::try_from(lines.unsigned_abs()).unwrap_or(u16::MAX);
                self.scroll = if lines < 0 {
                    self.scroll.saturating_sub(by)
                } else {
                    self.scroll.saturating_add(by)
                };
            }
            BenchMode::List => {
                for _ in 0..lines.unsigned_abs() {
                    self.move_selection(lines > 0);
                }
            }
            BenchMode::Form => {}
        }
    }
}

impl App {
    /// The BENCH tab's state.
    #[must_use]
    pub const fn bench(&self) -> &BenchState {
        &self.bench
    }

    /// Keep the detail view's scroll at most `top`, the line the last
    /// frame showed on top, so scrolling back up starts moving at once.
    pub(crate) fn clamp_bench_scroll(&mut self, top: u16) {
        if matches!(self.bench.mode, BenchMode::Detail(_)) {
            self.bench.scroll = self.bench.scroll.min(top);
        }
    }

    #[cfg(test)]
    pub(crate) fn bench_mut(&mut self) -> &mut BenchState {
        &mut self.bench
    }

    fn send_bench(&self, request: BenchRequest) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::Bench {
                agent_id: self.active_agent_id.clone(),
                request,
            });
        }
    }

    /// Ask for the benchmarks of the tree of the agent on screen and for
    /// its sessions.
    pub(in crate::app) fn request_benchmarks(&mut self) {
        self.send_bench(BenchRequest::List);
        self.send_bench(BenchRequest::Sessions);
    }

    /// Whether a running benchmark is on screen, in the BENCH tab or the
    /// progress panel, whose status and working runs shimmer frame by
    /// frame.
    #[must_use]
    pub fn bench_animating(&self) -> bool {
        let running = |view: &BenchView| view.status == BenchStatus::Running;
        let tab = self.bench.is_open()
            && match self.bench.mode {
                BenchMode::List => self.bench.benches.iter().any(running),
                BenchMode::Detail(_) => self.bench.shown().is_some_and(running),
                BenchMode::Form => false,
            };
        tab || self.bench_panel_shown().is_some_and(running)
    }

    pub(in crate::app) fn cmd_bench_list(
        &mut self,
        agent_id: &str,
        root: String,
        benches: Vec<BenchView>,
    ) {
        if agent_id == self.active_agent_id {
            for view in &benches {
                self.bench_panel.heard(view);
            }
            self.bench.list(root, benches);
        }
    }

    pub(in crate::app) fn cmd_bench_upsert(&mut self, view: BenchView) {
        self.bench_panel.heard(&view);
        self.bench.upsert(view);
    }

    pub(in crate::app) fn cmd_bench_notice(&mut self, notice: Result<String, String>) {
        let started = self.bench.mode == BenchMode::Form && notice.is_ok();
        self.notification = Some(match notice {
            Ok(text) => (text, NotificationType::Success),
            Err(problem) => (problem, NotificationType::Warning),
        });
        if started {
            self.bench.reset_form();
            self.bench.mode = BenchMode::List;
            self.bench.selected = 0;
        }
    }

    /// A key in the BENCH tab with nothing typed. Returns whether it used
    /// the key.
    pub(in crate::app) fn handle_bench_key(&mut self, key: KeyEvent) -> bool {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return false;
        }
        match self.bench.mode.clone() {
            BenchMode::List => self.bench_list_key(key),
            BenchMode::Detail(id) => self.bench_detail_key(key, &id),
            BenchMode::Form => self.bench_form_key(key),
        }
    }

    fn bench_list_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => self.show_chat_tab(ChatTab::Chat),
            KeyCode::Up | KeyCode::Char('k') => self.bench.move_selection(false),
            KeyCode::Down | KeyCode::Char('j') => self.bench.move_selection(true),
            KeyCode::Enter => {
                if let Some(view) = self.bench.benches.get(self.bench.selected) {
                    self.bench.mode = BenchMode::Detail(view.id.clone());
                    self.bench.scroll = 0;
                    self.bench.run = None;
                }
            }
            KeyCode::Char('n') => self.open_bench_form(),
            KeyCode::Char('r') => self.request_benchmarks(),
            KeyCode::Char('c') => self.send_bench(BenchRequest::Cancel),
            _ => return false,
        }
        true
    }

    fn bench_detail_key(&mut self, key: KeyEvent, id: &str) -> bool {
        let Some(view) = self.bench.shown() else {
            self.bench.mode = BenchMode::List;
            return true;
        };
        let runs = view.runs.len();
        let has_copies = view.status.has_copies() && view.status != BenchStatus::Running;
        let examples = !view.tasks.is_empty();
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
            KeyCode::Esc | KeyCode::Backspace => {
                self.bench.mode = BenchMode::List;
            }
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
            KeyCode::Char('d') if has_copies => {
                self.send_bench(BenchRequest::Diff { id, run });
            }
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
                Some(agent) => self.open_log_of(agent),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::BenchArm;

    fn view(id: &str) -> BenchView {
        BenchView {
            id: id.to_string(),
            root: "root".to_string(),
            name: format!("Compare {id}"),
            arms: [BenchArm::default(), BenchArm::default()],
            ..BenchView::default()
        }
    }

    #[test]
    fn the_list_is_newest_first_and_upserts_replace_in_place() {
        let mut state = BenchState::default();
        state.list("root".to_string(), vec![view("1"), view("2")]);
        assert_eq!(state.benches[0].id, "2");
        assert!(state.loaded());
        let mut changed = view("1");
        changed.title = "changed".to_string();
        state.upsert(changed);
        assert_eq!(state.benches.len(), 2);
        assert_eq!(state.benches[1].title, "changed");
        state.upsert(view("3"));
        assert_eq!(state.benches[0].id, "3");
    }

    #[test]
    fn each_tree_lists_its_own_benchmarks() {
        let mut app = App::new();
        app.active_agent_id = "user".to_string();
        app.cmd_bench_list("user", "root".to_string(), vec![view("1")]);
        app.bench.mode = BenchMode::Detail("1".to_string());

        let mut elsewhere = view("2");
        elsewhere.root = "other".to_string();
        app.cmd_bench_upsert(elsewhere.clone());
        assert_eq!(app.bench.benches.len(), 1, "another tree's benchmark");

        app.cmd_bench_list("someone else", "other".to_string(), vec![elsewhere.clone()]);
        assert_eq!(app.bench.benches[0].id, "1", "a list for another agent");

        app.active_agent_id = "other".to_string();
        app.cmd_bench_list("other", "other".to_string(), vec![elsewhere]);
        assert_eq!(app.bench.benches.len(), 1);
        assert_eq!(app.bench.benches[0].id, "2");
        assert_eq!(
            app.bench.mode,
            BenchMode::List,
            "the benchmark shown is not in this tree"
        );
        app.cmd_bench_upsert(view("3"));
        assert_eq!(app.bench.benches.len(), 1, "the first tree's benchmark");
    }

    #[test]
    fn a_benchmark_run_opens_on_its_own_tab() {
        use crate::events::{AgentSummary, UiCommand};
        let agent = |id: &str, bench_run: bool| AgentSummary {
            id: id.to_string(),
            name: id.to_string(),
            bench_run,
            ..AgentSummary::default()
        };
        let mut app = App::new();
        app.process_command(UiCommand::SetAgents(vec![
            agent("user", false),
            agent("other", false),
            agent("a-1", true),
        ]));
        app.process_command(UiCommand::SetActiveAgent("user".to_string()));
        app.show_chat_tab(ChatTab::Bench);

        // The run is picked in the agents list while BENCH is open.
        app.process_command(UiCommand::SetActiveAgent("a-1".to_string()));
        assert_eq!(
            app.chat_tab(),
            ChatTab::Chat,
            "a run never shown opens on its chat"
        );

        app.show_chat_tab(ChatTab::Bench);
        app.process_command(UiCommand::SetActiveAgent("a-1".to_string()));
        assert_eq!(
            app.chat_tab(),
            ChatTab::Bench,
            "the list refreshing does not take BENCH away"
        );

        app.process_command(UiCommand::SetActiveAgent("other".to_string()));
        assert_eq!(app.chat_tab(), ChatTab::Chat, "an agent never shown");

        app.process_command(UiCommand::SetActiveAgent("user".to_string()));
        assert_eq!(
            app.chat_tab(),
            ChatTab::Bench,
            "the agent the run was picked from"
        );

        app.process_command(UiCommand::SetActiveAgent("a-1".to_string()));
        assert_eq!(
            app.chat_tab(),
            ChatTab::Bench,
            "a run left on BENCH comes back on it"
        );
    }
}
