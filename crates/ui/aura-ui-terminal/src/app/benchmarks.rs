//! The chat panel's BENCH tab: benchmarks that run the same work on two
//! arms (Gate or a pinned model) and compare what they cost and how well
//! they did. The tab lists them, sets a new one up with a form, and shows
//! one's numbers; the event loop runs them as `/compare` comparisons.

use super::{App, ChatTab, NotificationType};
use crate::events::{
    BenchExample, BenchRequest, BenchSession, BenchSpec, BenchStatus, BenchView, BenchWork, UiEvent,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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

/// Rows of the new-benchmark form, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FormField {
    #[default]
    Work,
    Prompt,
    Tasks,
    A,
    B,
    Repeats,
    Check,
    Budget,
    Go,
}

impl FormField {
    pub const ALL: [Self; 9] = [
        Self::Work,
        Self::Prompt,
        Self::Tasks,
        Self::A,
        Self::B,
        Self::Repeats,
        Self::Check,
        Self::Budget,
        Self::Go,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Work => "Work",
            Self::Prompt => "Prompt",
            Self::Tasks => "Tasks",
            Self::A => "Arm A",
            Self::B => "Arm B",
            Self::Repeats => "Runs per arm",
            Self::Check => "Check",
            Self::Budget => "Budget (USD)",
            Self::Go => "",
        }
    }

    /// Whether its value is typed in the input line.
    #[must_use]
    pub const fn is_text(self) -> bool {
        matches!(self, Self::Prompt | Self::Check | Self::Budget)
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|f| *f == self).unwrap_or(0)
    }
}

/// What a new benchmark's runs work on, as the form offers it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WorkChoice<'a> {
    LastPrompt,
    Prompt,
    /// The built-in example coding tasks of a suite, or of every suite
    Examples(Option<&'a str>),
    Session(&'a BenchSession),
}

/// Most runs per arm the form offers.
const MAX_REPEATS: u32 = 10;

/// Example tasks the form starts at.
const DEFAULT_TASKS: usize = 5;

/// The new-benchmark form.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchForm {
    pub field: FormField,
    /// Index into [`BenchState::work_choices`]
    pub work: usize,
    pub prompt: String,
    /// How many of the chosen example tasks to run, the first ones first
    pub tasks: usize,
    /// `gate`, `gate:nodelegate` or a model id
    pub a: String,
    /// As `a`; empty for the strongest Opus
    pub b: String,
    pub repeats: u32,
    /// Empty for the project's check
    pub check: String,
    pub budget: String,
    /// What the set-up benchmark should cost, or why it can't run
    pub estimate: Option<Result<String, String>>,
    /// Whether an estimate was asked for and hasn't arrived
    pub waiting: bool,
}

impl Default for BenchForm {
    fn default() -> Self {
        Self {
            field: FormField::Work,
            work: 0,
            prompt: String::new(),
            tasks: DEFAULT_TASKS,
            a: "gate".to_string(),
            b: String::new(),
            repeats: 3,
            check: String::new(),
            budget: String::new(),
            estimate: None,
            waiting: false,
        }
    }
}

impl BenchForm {
    /// Whether the benchmark it set up can start.
    #[must_use]
    pub const fn ready(&self) -> bool {
        matches!(self.estimate, Some(Ok(_)))
    }
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
}

impl BenchState {
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    pub(super) fn set_open(&mut self, open: bool) {
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

    /// The suites of the example tasks, in the order a benchmark takes
    /// them.
    #[must_use]
    pub fn suites(&self) -> Vec<&str> {
        let mut suites: Vec<&str> = Vec::new();
        for example in &self.examples {
            if !suites.contains(&example.suite.as_str()) {
                suites.push(&example.suite);
            }
        }
        suites
    }

    /// The example tasks of `suite`, or of every suite.
    #[must_use]
    pub fn examples_of(&self, suite: Option<&str>) -> Vec<&BenchExample> {
        self.examples
            .iter()
            .filter(|e| suite.is_none_or(|suite| e.suite == suite))
            .collect()
    }

    /// What the form's work row cycles through: every example task, then
    /// each suite's on their own.
    #[must_use]
    pub fn work_choices(&self) -> Vec<WorkChoice<'_>> {
        let mut choices = vec![
            WorkChoice::LastPrompt,
            WorkChoice::Prompt,
            WorkChoice::Examples(None),
        ];
        let suites = self.suites();
        if suites.len() > 1 {
            choices.extend(suites.into_iter().map(|s| WorkChoice::Examples(Some(s))));
        }
        choices.extend(self.sessions.iter().map(WorkChoice::Session));
        choices
    }

    /// How many example tasks the chosen work has; 0 when it is not
    /// example tasks.
    #[must_use]
    pub fn example_count(&self) -> usize {
        match self.work() {
            WorkChoice::Examples(suite) => self.examples_of(suite).len(),
            _ => 0,
        }
    }

    /// The work the form has chosen.
    #[must_use]
    pub fn work(&self) -> WorkChoice<'_> {
        self.work_choices()
            .get(self.form.work)
            .copied()
            .unwrap_or(WorkChoice::LastPrompt)
    }

    fn list(&mut self, root: String, mut benches: Vec<BenchView>) {
        if self.root.as_ref() != Some(&root) {
            self.selected = 0;
            self.run = None;
            self.scroll = 0;
        }
        benches.reverse();
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
    pub(super) fn scroll_by(&mut self, lines: isize) {
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

/// `values` cycled from `current` one step forward or back.
fn cycle(values: &[String], current: &str, forward: bool) -> String {
    if values.is_empty() {
        return current.to_string();
    }
    let at = values.iter().position(|v| v == current);
    let next = match (at, forward) {
        (None, _) => 0,
        (Some(i), true) => (i + 1) % values.len(),
        (Some(i), false) => (i + values.len() - 1) % values.len(),
    };
    values[next].clone()
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
    pub(super) fn request_benchmarks(&mut self) {
        self.send_bench(BenchRequest::List);
        self.send_bench(BenchRequest::Sessions);
    }

    /// What the form's arm rows cycle through: Gate both ways, then every
    /// model that can run. Arm B also offers its default.
    #[must_use]
    pub fn bench_arm_choices(&self, arm_b: bool) -> Vec<String> {
        let mut choices = Vec::new();
        if arm_b {
            choices.push(String::new());
        }
        choices.push("gate".to_string());
        choices.push("gate:nodelegate".to_string());
        choices.extend(
            self.models
                .iter()
                .filter(|m| m.available && m.id != "gate" && (!m.local || m.installed))
                .map(|m| m.id.clone()),
        );
        choices
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

    pub(super) fn cmd_bench_list(&mut self, agent_id: &str, root: String, benches: Vec<BenchView>) {
        if agent_id == self.active_agent_id {
            for view in &benches {
                self.bench_panel.heard(view);
            }
            self.bench.list(root, benches);
        }
    }

    pub(super) fn cmd_bench_upsert(&mut self, view: BenchView) {
        self.bench_panel.heard(&view);
        self.bench.upsert(view);
    }

    pub(super) fn cmd_bench_sessions(
        &mut self,
        agent_id: &str,
        sessions: Vec<BenchSession>,
        examples: Vec<BenchExample>,
    ) {
        if agent_id == self.active_agent_id {
            self.bench.sessions = sessions;
            let choices = self.bench.work_choices().len();
            self.bench.form.work = self.bench.form.work.min(choices.saturating_sub(1));
            if !examples.is_empty() {
                self.bench.form.tasks = self.bench.form.tasks.clamp(1, examples.len());
            }
            self.bench.examples = examples;
        }
    }

    pub(super) fn cmd_bench_estimate(&mut self, agent_id: &str, estimate: Result<String, String>) {
        if agent_id == self.active_agent_id {
            self.bench.form.waiting = false;
            if estimate.is_ok() {
                self.bench.form.field = FormField::Go;
            }
            self.bench.form.estimate = Some(estimate);
        }
    }

    pub(super) fn cmd_bench_notice(&mut self, notice: Result<String, String>) {
        let started = self.bench.mode == BenchMode::Form && notice.is_ok();
        self.notification = Some(match notice {
            Ok(text) => (text, NotificationType::Success),
            Err(problem) => (problem, NotificationType::Warning),
        });
        if started {
            self.bench.form = BenchForm::default();
            self.bench.mode = BenchMode::List;
            self.bench.selected = 0;
        }
    }

    /// Enter with text in the input line while the form's focused row is
    /// typed: the text becomes its value. Returns whether it did.
    pub(super) fn bench_fill_field(&mut self) -> bool {
        let form = &mut self.bench.form;
        if self.bench.mode != BenchMode::Form || !form.field.is_text() || self.input.is_empty() {
            return false;
        }
        let text = std::mem::take(&mut self.input);
        self.cursor_pos = 0;
        match form.field {
            FormField::Prompt => {
                form.prompt = text;
                form.work = 1;
            }
            FormField::Check => form.check = text.trim().to_string(),
            FormField::Budget => form.budget = text.trim().trim_start_matches('$').to_string(),
            _ => {}
        }
        form.estimate = None;
        true
    }

    /// A key in the BENCH tab with nothing typed. Returns whether it used
    /// the key.
    pub(super) fn handle_bench_key(&mut self, key: KeyEvent) -> bool {
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

    /// Open the form, asking for the sessions it offers.
    pub(super) fn open_bench_form(&mut self) {
        self.bench.mode = BenchMode::Form;
        self.bench.form.estimate = None;
        self.bench.form.waiting = false;
        self.send_bench(BenchRequest::Sessions);
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
            KeyCode::Char(c @ ('a' | 'b')) if has_copies => {
                let arm = usize::from(c == 'b');
                self.send_bench(BenchRequest::Pick { id, arm });
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
            _ => return false,
        }
        true
    }

    fn bench_form_key(&mut self, key: KeyEvent) -> bool {
        let field = self.bench.form.field;
        match key.code {
            KeyCode::Esc => {
                if self.bench.form.ready() {
                    self.send_bench(BenchRequest::Cancel);
                }
                self.bench.form.estimate = None;
                self.bench.mode = BenchMode::List;
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => self.move_form_field(false),
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => self.move_form_field(true),
            KeyCode::Left | KeyCode::Char('h') => self.change_form_value(false),
            KeyCode::Right | KeyCode::Char('l') => self.change_form_value(true),
            KeyCode::Delete | KeyCode::Backspace if field.is_text() => {
                let form = &mut self.bench.form;
                match field {
                    FormField::Prompt => form.prompt.clear(),
                    FormField::Check => form.check.clear(),
                    FormField::Budget => form.budget.clear(),
                    _ => {}
                }
                form.estimate = None;
            }
            KeyCode::Enter if field == FormField::Go => self.submit_bench_form(),
            KeyCode::Enter => self.move_form_field(true),
            _ => return false,
        }
        true
    }

    fn move_form_field(&mut self, down: bool) {
        let form = &mut self.bench.form;
        let at = form.field.index();
        let next = if down {
            (at + 1).min(FormField::ALL.len() - 1)
        } else {
            at.saturating_sub(1)
        };
        form.field = FormField::ALL[next];
    }

    fn change_form_value(&mut self, forward: bool) {
        let field = self.bench.form.field;
        match field {
            FormField::Work => {
                let count = self.bench.work_choices().len();
                let form = &mut self.bench.form;
                form.work = if forward {
                    (form.work + 1) % count
                } else {
                    (form.work + count - 1) % count
                };
                let most = self.bench.example_count();
                if most > 0 {
                    self.bench.form.tasks = self.bench.form.tasks.clamp(1, most);
                }
            }
            FormField::A | FormField::B => {
                let choices = self.bench_arm_choices(field == FormField::B);
                let form = &mut self.bench.form;
                let value = if field == FormField::A {
                    &mut form.a
                } else {
                    &mut form.b
                };
                *value = cycle(&choices, value, forward);
            }
            FormField::Tasks => {
                let most = self.bench.example_count();
                if most == 0 {
                    return;
                }
                let form = &mut self.bench.form;
                form.tasks = if forward {
                    (form.tasks + 1).min(most)
                } else {
                    form.tasks.saturating_sub(1).max(1)
                };
            }
            FormField::Repeats => {
                let form = &mut self.bench.form;
                form.repeats = if forward {
                    (form.repeats + 1).min(MAX_REPEATS)
                } else {
                    form.repeats.saturating_sub(1).max(1)
                };
            }
            _ => return,
        }
        self.bench.form.estimate = None;
    }

    /// Enter on the form's last row: ask what the benchmark should cost,
    /// or start it once that came back.
    fn submit_bench_form(&mut self) {
        if self.bench.form.waiting {
            return;
        }
        if self.bench.form.ready() {
            self.send_bench(BenchRequest::Start);
            return;
        }
        let form = self.bench.form.clone();
        let work = match self.bench.work() {
            WorkChoice::LastPrompt => Some(BenchWork::LastPrompt),
            WorkChoice::Prompt if form.prompt.trim().is_empty() => None,
            WorkChoice::Prompt => Some(BenchWork::Prompt(form.prompt.clone())),
            WorkChoice::Examples(suite) => Some(BenchWork::Examples {
                suite: suite.map(str::to_string),
                tasks: form.tasks.clamp(1, self.bench.example_count().max(1)),
            }),
            WorkChoice::Session(session) if session.current => Some(BenchWork::CurrentSession),
            WorkChoice::Session(session) => Some(BenchWork::Session(session.started_ms)),
        };
        let Some(work) = work else {
            self.bench.form.field = FormField::Prompt;
            self.notification = Some((
                "Type the prompt in the input line and press Enter".to_string(),
                NotificationType::Warning,
            ));
            return;
        };
        let budget_usd = match form.budget.trim() {
            "" => None,
            text => match text.parse::<f64>() {
                Ok(usd) if usd > 0.0 => Some(usd),
                _ => {
                    self.notification = Some((
                        format!("Budget {text:?} is not an amount in USD"),
                        NotificationType::Warning,
                    ));
                    return;
                }
            },
        };
        let spec = BenchSpec {
            work,
            a: form.a.clone(),
            b: form.b.clone(),
            repeats: form.repeats,
            check: Some(form.check.clone()).filter(|c| !c.is_empty()),
            budget_usd,
        };
        self.bench.form.waiting = true;
        self.send_bench(BenchRequest::Prepare(spec));
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
    fn values_cycle_both_ways() {
        let values: Vec<String> = ["gate", "x", "y"].iter().map(ToString::to_string).collect();
        assert_eq!(cycle(&values, "gate", true), "x");
        assert_eq!(cycle(&values, "gate", false), "y");
        assert_eq!(cycle(&values, "unknown", true), "gate");
    }

    #[test]
    fn work_choices_offer_the_sessions_after_the_prompts() {
        let mut state = BenchState {
            sessions: vec![BenchSession {
                started_ms: 5,
                title: "fix it".to_string(),
                prompts: 3,
                cost_usd: Some(0.4),
                models: Vec::new(),
                current: true,
            }],
            ..BenchState::default()
        };
        let choices = state.work_choices();
        assert_eq!(choices.len(), 4);
        assert_eq!(choices[2], WorkChoice::Examples(None));
        assert!(matches!(choices[3], WorkChoice::Session(s) if s.started_ms == 5));
        state.form.work = 3;
        assert!(matches!(state.work(), WorkChoice::Session(_)));
    }

    #[test]
    fn picking_a_run_shows_its_chat_once() {
        use super::super::ChatTab;
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

        app.process_command(UiCommand::SetActiveAgent("other".to_string()));
        assert_eq!(app.chat_tab(), ChatTab::Bench, "another agent keeps BENCH");

        app.process_command(UiCommand::SetActiveAgent("a-1".to_string()));
        assert_eq!(app.chat_tab(), ChatTab::Chat, "a run is picked to watch it");

        app.show_chat_tab(ChatTab::Bench);
        app.process_command(UiCommand::SetActiveAgent("a-1".to_string()));
        assert_eq!(
            app.chat_tab(),
            ChatTab::Bench,
            "the list refreshing does not take BENCH away again"
        );
    }

    fn example(suite: &str, title: &str) -> BenchExample {
        BenchExample {
            suite: suite.to_string(),
            title: title.to_string(),
        }
    }

    #[test]
    fn the_tasks_row_counts_from_one_to_the_example_tasks() {
        let mut app = App::new();
        let id = app.active_agent_id.clone();
        let titles: Vec<BenchExample> = (1..=7)
            .map(|i| example("coding", &format!("task {i}")))
            .collect();
        app.cmd_bench_sessions(&id, Vec::new(), titles);
        assert_eq!(app.bench.form.tasks, DEFAULT_TASKS);
        app.bench.form.work = 2;
        app.bench.form.field = FormField::Tasks;
        for _ in 0..5 {
            app.change_form_value(true);
        }
        assert_eq!(app.bench.form.tasks, 7, "stops at the last task");
        for _ in 0..10 {
            app.change_form_value(false);
        }
        assert_eq!(app.bench.form.tasks, 1, "and at one");
        app.cmd_bench_sessions(&id, Vec::new(), vec![example("coding", "only")]);
        app.bench.form.tasks = 4;
        app.cmd_bench_sessions(&id, Vec::new(), vec![example("coding", "only")]);
        assert_eq!(app.bench.form.tasks, 1, "clamped to what there is");
    }

    #[test]
    fn each_suite_can_be_run_on_its_own() {
        let mut app = App::new();
        let id = app.active_agent_id.clone();
        let mut examples: Vec<BenchExample> = (1..=5)
            .map(|i| example("coding", &format!("easy {i}")))
            .collect();
        examples.extend((1..=3).map(|i| example("gate_hard", &format!("hard {i}"))));
        app.cmd_bench_sessions(&id, Vec::new(), examples);
        let choices = app.bench.work_choices();
        assert_eq!(
            choices[2..],
            [
                WorkChoice::Examples(None),
                WorkChoice::Examples(Some("coding")),
                WorkChoice::Examples(Some("gate_hard")),
            ]
        );

        app.bench.form.field = FormField::Work;
        app.bench.form.work = 3;
        app.change_form_value(true);
        assert_eq!(app.bench.work(), WorkChoice::Examples(Some("gate_hard")));
        assert_eq!(app.bench.form.tasks, 3, "clamped to the suite's tasks");
        app.bench.form.field = FormField::Tasks;
        app.change_form_value(true);
        assert_eq!(app.bench.form.tasks, 3, "stops at the suite's last task");

        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        app.event_tx = Some(tx);
        app.submit_bench_form();
        let sent = std::iter::from_fn(|| rx.try_recv().ok()).find_map(|event| match event {
            UiEvent::Bench {
                request: BenchRequest::Prepare(spec),
                ..
            } => Some(spec.work),
            _ => None,
        });
        assert_eq!(
            sent,
            Some(BenchWork::Examples {
                suite: Some("gate_hard".to_string()),
                tasks: 3,
            })
        );
    }
}
