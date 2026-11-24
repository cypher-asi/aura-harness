//! The chat panel's BENCH tab: benchmarks that run the same work on two
//! arms (Gate, a pinned model, or a model per difficulty) and compare
//! what they cost and how well they did. The tab lists them, sets a new
//! one up with a form, and shows one's numbers; the event loop runs them
//! as `/compare` comparisons. Enter on a difficulty's row of the form
//! opens the `/model` selector to pick its model.

use super::{App, AppState, ChatTab, NotificationType, PickerTarget};
use crate::events::{
    BenchExample, BenchRequest, BenchSession, BenchSpec, BenchStatus, BenchView, BenchWork, UiEvent,
};
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

/// Rows of the new-benchmark form, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FormField {
    #[default]
    Work,
    Prompt,
    Tasks,
    A,
    /// The models of arm A's easy, medium and hard work, when it runs work
    /// by its difficulty
    AEasy,
    AMedium,
    AHard,
    B,
    BEasy,
    BMedium,
    BHard,
    /// The model that classifies each run's prompt; [`AUTO_CLASSIFIER`]
    /// for Gate's choice
    Classifier,
    Repeats,
    Check,
    Budget,
    Go,
}

impl FormField {
    pub const ALL: [Self; 16] = [
        Self::Work,
        Self::Prompt,
        Self::Tasks,
        Self::A,
        Self::AEasy,
        Self::AMedium,
        Self::AHard,
        Self::B,
        Self::BEasy,
        Self::BMedium,
        Self::BHard,
        Self::Classifier,
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
            Self::AEasy | Self::BEasy => "  Easy",
            Self::AMedium | Self::BMedium => "  Medium",
            Self::AHard | Self::BHard => "  Hard",
            Self::Classifier => "Classifier",
            Self::Repeats => "Runs per arm",
            Self::Check => "Check",
            Self::Budget => "Budget (USD)",
            Self::Go => "",
        }
    }

    /// Whether its value is typed in the input line.
    #[must_use]
    pub const fn is_text(self) -> bool {
        matches!(self, Self::Prompt | Self::Check | Self::Budget) || self.holds_model()
    }

    /// Whether the row holds a model, picked in the `/model` selector:
    /// a difficulty's row, or the classifier's.
    #[must_use]
    pub const fn holds_model(self) -> bool {
        self.picker_target().is_some()
    }

    /// What the model selector picks for, opened on this row; `None` for
    /// a row that holds no model.
    #[must_use]
    pub const fn picker_target(self) -> Option<PickerTarget> {
        match self.difficulty() {
            Some((arm, difficulty)) => Some(PickerTarget::BenchDifficulty { arm, difficulty }),
            None if matches!(self, Self::Classifier) => Some(PickerTarget::BenchClassifier),
            None => None,
        }
    }

    /// The row the model selector was opened on for `target`; `None` when
    /// it picks the session's model.
    #[must_use]
    pub const fn of_picker(target: PickerTarget) -> Option<Self> {
        match target {
            PickerTarget::Session => None,
            PickerTarget::BenchDifficulty { arm, difficulty } => {
                Some(Self::of_difficulty(arm, difficulty))
            }
            PickerTarget::BenchClassifier => Some(Self::Classifier),
        }
    }

    /// The arm (0 for A, 1 for B) and difficulty (0 easy, 1 medium, 2
    /// hard) of a row naming the model of one difficulty.
    #[must_use]
    pub const fn difficulty(self) -> Option<(usize, usize)> {
        match self {
            Self::AEasy => Some((0, 0)),
            Self::AMedium => Some((0, 1)),
            Self::AHard => Some((0, 2)),
            Self::BEasy => Some((1, 0)),
            Self::BMedium => Some((1, 1)),
            Self::BHard => Some((1, 2)),
            _ => None,
        }
    }

    /// The row naming the model of `difficulty` (0 easy, 1 medium, 2 hard)
    /// of `arm`.
    const fn of_difficulty(arm: usize, difficulty: usize) -> Self {
        match (arm, difficulty) {
            (0, 0) => Self::AEasy,
            (0, 1) => Self::AMedium,
            (0, _) => Self::AHard,
            (_, 0) => Self::BEasy,
            (_, 1) => Self::BMedium,
            _ => Self::BHard,
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|f| *f == self).unwrap_or(0)
    }
}

/// The arm value that runs each piece of work on the model the form gives
/// its difficulty.
pub const BY_DIFFICULTY: &str = "by difficulty";

/// The classifier row's value for Gate's automatic choice: the cheapest
/// fast model the user can run.
pub const AUTO_CLASSIFIER: &str = "auto";

/// The classifier row's starting value: Claude Haiku, sent as
/// `--classifier haiku` is when the form can't name the Haiku the user
/// runs. Cycling or the selector still pick another, [`AUTO_CLASSIFIER`]
/// included.
pub const DEFAULT_CLASSIFIER: &str = "haiku";

/// The names of the difficulties, easiest first, as the arm string has
/// them.
pub const DIFFICULTIES: [&str; 3] = ["easy", "medium", "hard"];

/// The arm string of a by-difficulty arm whose easy, medium and hard work
/// runs on `models`: `difficulty:easy=<id>,medium=<id>,hard=<id>`, as
/// `/compare --a` takes it.
#[must_use]
pub fn difficulty_arm(models: &[String; 3]) -> String {
    let entries: Vec<String> = DIFFICULTIES
        .iter()
        .zip(models)
        .map(|(difficulty, model)| format!("{difficulty}={}", model.trim()))
        .collect();
    format!("difficulty:{}", entries.join(","))
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

/// The built-in SWE-bench Pro Hard suite that new benchmarks run.
const DEFAULT_SUITE: &str = "swe_bench_pro_hard";

/// Arm B's starting value: Opus 5.5. The form swaps it for the id of the
/// Opus 5.5 the user can run ([`App::default_bench_against`]) once the
/// model list is in, so it resolves on any route and cycles with the
/// other models.
pub const DEFAULT_AGAINST: &str = "opus-5-5";

/// Whether `id` names Opus 5.5 on any route: `claude/claude-opus-5-5`,
/// `aura-claude-opus-5-5`, `openrouter/anthropic/claude-opus-5.5`. Variants
/// such as `claude-opus-5-5-fast` are not it.
fn is_opus_5_5(id: &str) -> bool {
    let name = id.rsplit('/').next().unwrap_or(id).to_ascii_lowercase();
    let name = name.replace('.', "-");
    let name = name.strip_prefix("aura-").unwrap_or(&name);
    name.strip_prefix("claude-").unwrap_or(name) == DEFAULT_AGAINST
}

/// One attempt per task on each arm, so repeated runs are opt-in.
const DEFAULT_REPEATS: u32 = 1;

/// A work selection waiting for the example suites to arrive.
const DEFAULT_WORK: usize = usize::MAX;

/// The new-benchmark form.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchForm {
    pub field: FormField,
    /// Index into [`BenchState::work_choices`], or [`DEFAULT_WORK`] while
    /// the examples that determine the default suite are loading
    pub work: usize,
    pub prompt: String,
    /// How many of the chosen example tasks to run, the first ones first
    pub tasks: usize,
    /// `gate`, `gate:nodelegate`, [`BY_DIFFICULTY`] or a model id
    pub a: String,
    /// As `a`; [`DEFAULT_AGAINST`] until the form names the Opus 5.5 the
    /// user runs, empty for the strongest Opus
    pub b: String,
    /// The models of arm A's easy, medium and hard work when `a` is
    /// [`BY_DIFFICULTY`]; empty until chosen
    pub a_models: [String; 3],
    /// As `a_models`, for arm B; empty until chosen
    pub b_models: [String; 3],
    /// The model that classifies each run's prompt, or
    /// [`AUTO_CLASSIFIER`] for Gate's choice; [`DEFAULT_CLASSIFIER`]
    /// until changed
    pub classifier: String,
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
            work: DEFAULT_WORK,
            prompt: String::new(),
            tasks: 0,
            a: "gate".to_string(),
            b: DEFAULT_AGAINST.to_string(),
            a_models: Default::default(),
            b_models: Default::default(),
            classifier: DEFAULT_CLASSIFIER.to_string(),
            repeats: DEFAULT_REPEATS,
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

    /// What arm `arm` (0 for A, 1 for B) runs on.
    #[must_use]
    pub fn arm(&self, arm: usize) -> &str {
        if arm == 0 {
            &self.a
        } else {
            &self.b
        }
    }

    /// The models of arm `arm`'s easy, medium and hard work.
    #[must_use]
    pub const fn models(&self, arm: usize) -> &[String; 3] {
        if arm == 0 {
            &self.a_models
        } else {
            &self.b_models
        }
    }

    fn models_mut(&mut self, arm: usize) -> &mut [String; 3] {
        if arm == 0 {
            &mut self.a_models
        } else {
            &mut self.b_models
        }
    }

    /// Whether `field` is on the form: a difficulty's row only while its
    /// arm runs work by difficulty.
    #[must_use]
    pub fn shown(&self, field: FormField) -> bool {
        field
            .difficulty()
            .is_none_or(|(arm, _)| self.arm(arm) == BY_DIFFICULTY)
    }

    /// The model of the difficulty row `field`, when it is one.
    #[must_use]
    pub fn difficulty_model(&self, field: FormField) -> Option<&str> {
        let (arm, difficulty) = field.difficulty()?;
        Some(self.models(arm)[difficulty].as_str())
    }

    /// The model on `field`, when it is a row that holds one: a
    /// difficulty's model, or the classifier.
    #[must_use]
    pub fn row_model(&self, field: FormField) -> Option<&str> {
        match field {
            FormField::Classifier => Some(&self.classifier),
            _ => self.difficulty_model(field),
        }
    }

    fn row_model_mut(&mut self, field: FormField) -> Option<&mut String> {
        match (field, field.difficulty()) {
            (FormField::Classifier, _) => Some(&mut self.classifier),
            (_, Some((arm, difficulty))) => Some(&mut self.models_mut(arm)[difficulty]),
            (_, None) => None,
        }
    }

    /// The model the classifier row names; `None` for Gate's choice.
    #[must_use]
    pub fn classifier_model(&self) -> Option<String> {
        let model = self.classifier.trim();
        (!model.is_empty() && model != AUTO_CLASSIFIER).then(|| model.to_string())
    }

    /// What arm `arm` is sent as: its value, or the arm string of its
    /// models when it runs work by difficulty. A by-difficulty arm missing
    /// a model gives the row that lacks it.
    ///
    /// # Errors
    /// The difficulty row with no model yet.
    pub fn arm_value(&self, arm: usize) -> Result<String, FormField> {
        let value = self.arm(arm);
        if value != BY_DIFFICULTY {
            return Ok(value.to_string());
        }
        let models = self.models(arm);
        match models.iter().position(|m| m.trim().is_empty()) {
            Some(missing) => Err(FormField::of_difficulty(arm, missing)),
            None => Ok(difficulty_arm(models)),
        }
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

    pub(super) fn set_open(&mut self, open: bool) {
        self.open = open;
    }

    #[must_use]
    pub const fn loaded(&self) -> bool {
        self.loaded
    }

    /// Where the form was drawn this frame (`area`) and the screen row of
    /// each of its rows; `None` when the tab shows something else.
    fn set_form_rows(&mut self, area: Option<Rect>, rows: Vec<(u16, FormField)>) {
        self.form_area = area;
        self.form_rows = rows;
    }

    /// The form's row drawn at the screen cell, in the last frame.
    fn form_field_at(&self, column: u16, row: u16) -> Option<FormField> {
        let area = self.form_area?;
        let inside = column >= area.x
            && column < area.x.saturating_add(area.width)
            && row >= area.y
            && row < area.y.saturating_add(area.height);
        inside
            .then(|| self.form_rows.iter().find(|(y, _)| *y == row))
            .flatten()
            .map(|(_, field)| *field)
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

    /// Resolve the default work choice and its full task count from the
    /// available examples, preferring [`DEFAULT_SUITE`].
    fn default_work(examples: &[BenchExample]) -> (WorkChoice<'_>, usize) {
        let tasks = examples
            .iter()
            .filter(|example| example.suite == DEFAULT_SUITE)
            .count();
        if tasks > 0 {
            (WorkChoice::Examples(Some(DEFAULT_SUITE)), tasks)
        } else if examples.is_empty() {
            (WorkChoice::LastPrompt, 0)
        } else {
            (WorkChoice::Examples(None), examples.len())
        }
    }

    /// Start the form on its shared defaults, including the full default
    /// suite when its examples are already cached.
    fn reset_form(&mut self) {
        self.form = BenchForm::default();
        if self.examples.is_empty() {
            return;
        }
        let (work, tasks) = Self::default_work(&self.examples);
        self.form.work = self
            .work_choices()
            .iter()
            .position(|choice| *choice == work)
            .unwrap_or(DEFAULT_WORK);
        self.form.tasks = tasks;
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
        if suites.len() > 1 || suites.contains(&DEFAULT_SUITE) {
            choices.extend(suites.into_iter().map(|s| WorkChoice::Examples(Some(s))));
        }
        choices.extend(self.sessions.iter().map(WorkChoice::Session));
        choices
    }

    /// The index of the work the form has chosen, or the default work while
    /// its sentinel is waiting for examples.
    fn work_index(&self) -> usize {
        if self.form.work != DEFAULT_WORK {
            return self.form.work;
        }
        let (work, _) = Self::default_work(&self.examples);
        self.work_choices()
            .iter()
            .position(|choice| *choice == work)
            .unwrap_or(0)
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

    /// Clamp the task count to the chosen work without changing a valid
    /// count the user entered.
    fn clamp_tasks(&mut self) {
        let most = self.example_count();
        if most > 0 {
            self.form.tasks = self.form.tasks.clamp(1, most);
        }
    }

    /// The work the form has chosen.
    #[must_use]
    pub fn work(&self) -> WorkChoice<'_> {
        self.work_choices()
            .get(self.work_index())
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

    /// Where the BENCH form was drawn this frame and the screen row of
    /// each of its rows, for clicks; `None` when it is not shown.
    pub(crate) fn set_bench_form_rows(&mut self, area: Option<Rect>, rows: Vec<(u16, FormField)>) {
        self.bench.set_form_rows(area, rows);
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

    /// What the form's arm rows cycle through: Gate both ways, a model
    /// per difficulty, then every model that can run. Arm B also offers
    /// its default.
    #[must_use]
    pub fn bench_arm_choices(&self, arm_b: bool) -> Vec<String> {
        let mut choices = Vec::new();
        if arm_b {
            choices.push(String::new());
        }
        choices.push("gate".to_string());
        choices.push("gate:nodelegate".to_string());
        choices.push(BY_DIFFICULTY.to_string());
        choices.extend(self.bench_model_choices());
        choices
    }

    /// Every model that can run, on any route: what a difficulty's row
    /// cycles through.
    #[must_use]
    pub fn bench_model_choices(&self) -> Vec<String> {
        self.models
            .iter()
            .filter(|m| m.available && m.id != "gate" && (!m.local || m.installed))
            .map(|m| m.id.clone())
            .collect()
    }

    /// The classifier a new benchmark starts on: the first Haiku that can
    /// run, else [`DEFAULT_CLASSIFIER`] for the server to find.
    #[must_use]
    pub fn default_bench_classifier(&self) -> String {
        self.bench_model_choices()
            .into_iter()
            .find(|id| id.to_ascii_lowercase().contains(DEFAULT_CLASSIFIER))
            .unwrap_or_else(|| DEFAULT_CLASSIFIER.to_string())
    }

    /// Arm B's default: the Opus 5.5 the user can run, on their Claude
    /// plan when they have one, else empty for the strongest Opus there
    /// is, which the server picks.
    #[must_use]
    pub fn default_bench_against(&self) -> String {
        let opus: Vec<String> = self
            .bench_model_choices()
            .into_iter()
            .filter(|id| is_opus_5_5(id))
            .collect();
        opus.iter()
            .find(|id| id.starts_with("claude/"))
            .or_else(|| opus.first())
            .cloned()
            .unwrap_or_default()
    }

    /// Put the runnable Opus 5.5 in place of arm B's [`DEFAULT_AGAINST`],
    /// once the model list is in; an arm the user chose is left alone.
    fn settle_bench_against(&mut self) {
        if self.bench.form.b == DEFAULT_AGAINST && !self.models.is_empty() {
            self.bench.form.b = self.default_bench_against();
        }
    }

    /// Open the model selector, as `/model` does, to pick the model of the
    /// form's row `field` (a difficulty's, or the classifier's), filtered
    /// by `filter` and on the row's model when it has one.
    pub(super) fn open_bench_model_picker(&mut self, field: FormField, filter: &str) {
        let Some(target) = field.picker_target() else {
            return;
        };
        if self.models.is_empty() {
            self.notification = Some((
                "Model list not loaded yet".to_string(),
                NotificationType::Warning,
            ));
            return;
        }
        let current = self
            .bench
            .form
            .row_model(field)
            .unwrap_or_default()
            .to_string();
        self.model_picker
            .open_for(&self.models, &current, target, filter);
        self.state = AppState::SelectingModel;
    }

    /// Put the model `id` picked in the selector on the form's row
    /// `field`, and go on to the next row. A model that can't run yet is
    /// refused with what it needs, and the selector stays open.
    pub(super) fn pick_bench_model(&mut self, field: FormField, id: &str) {
        let needs = self.models.iter().find(|m| m.id == id).and_then(|m| {
            if m.local && !m.installed {
                Some(format!("{} is not pulled yet", m.label))
            } else if !m.available {
                let login = match m.provider.as_deref() {
                    Some(provider) if provider != "zos" => format!("/login {provider}"),
                    _ => "/login".to_string(),
                };
                Some(format!("{} needs a sign-in first: {login}", m.label))
            } else {
                None
            }
        });
        if let Some(needs) = needs {
            self.notification = Some((needs, NotificationType::Warning));
            return;
        }
        let form = &mut self.bench.form;
        let Some(model) = form.row_model_mut(field) else {
            return;
        };
        *model = id.to_string();
        form.estimate = None;
        form.field = field;
        self.state = AppState::Idle;
        self.move_form_field(true);
    }

    /// The model the selector marks as chosen: the session's, or the
    /// benchmark row's it was opened for.
    #[must_use]
    pub fn picker_marked(&self) -> &str {
        match FormField::of_picker(self.model_picker.target()) {
            None => &self.current_model,
            Some(field) => self.bench.form.row_model(field).unwrap_or_default(),
        }
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
            self.bench.examples = examples;
            if self.bench.form.work == DEFAULT_WORK && !self.bench.examples.is_empty() {
                self.bench.form.work = self.bench.work_index();
                self.bench.form.tasks = self.bench.example_count();
            } else if self.bench.form.work != DEFAULT_WORK {
                let choices = self.bench.work_choices().len();
                self.bench.form.work = self.bench.form.work.min(choices.saturating_sub(1));
            }
            self.bench.clamp_tasks();
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
            self.bench.reset_form();
            self.bench.mode = BenchMode::List;
            self.bench.selected = 0;
        }
    }

    /// Enter with text in the input line while the form's focused row is
    /// typed: the text becomes its value. Returns whether it did.
    pub(super) fn bench_fill_field(&mut self) -> bool {
        let field = self.bench.form.field;
        if self.bench.mode != BenchMode::Form || !field.is_text() || self.input.is_empty() {
            return false;
        }
        let text = std::mem::take(&mut self.input);
        self.cursor_pos = 0;
        if field.holds_model() {
            self.open_bench_model_picker(field, &text);
            return true;
        }
        let form = &mut self.bench.form;
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
        if self.bench.form.classifier == DEFAULT_CLASSIFIER {
            // Name the Haiku the user runs, now the model list is likely
            // in; a classifier the user chose is left alone.
            self.bench.form.classifier = self.default_bench_classifier();
        }
        self.settle_bench_against();
        self.bench.form.estimate = None;
        self.bench.form.waiting = false;
        self.send_bench(BenchRequest::Sessions);
    }

    /// A left click in the BENCH tab: on a row of the form it goes to the
    /// row, opens the model selector on a difficulty's row, and on the
    /// last row asks for the estimate or starts the benchmark, as Enter
    /// does there. Returns whether it hit a row.
    pub(crate) fn click_bench(&mut self, column: u16, row: u16) -> bool {
        if self.bench.mode != BenchMode::Form {
            return false;
        }
        let Some(field) = self.bench.form_field_at(column, row) else {
            return false;
        };
        self.bench.form.field = field;
        if field.holds_model() {
            self.open_bench_model_picker(field, "");
        } else if field == FormField::Go {
            self.submit_bench_form();
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
                let default_classifier = self.default_bench_classifier();
                let form = &mut self.bench.form;
                match field {
                    FormField::Prompt => form.prompt.clear(),
                    FormField::Check => form.check.clear(),
                    FormField::Budget => form.budget.clear(),
                    FormField::Classifier => form.classifier = default_classifier,
                    _ => {
                        if let Some(model) = form.row_model_mut(field) {
                            model.clear();
                        }
                    }
                }
                form.estimate = None;
            }
            KeyCode::Enter if field == FormField::Go => self.submit_bench_form(),
            KeyCode::Enter if field.holds_model() => self.open_bench_model_picker(field, ""),
            KeyCode::Enter => self.move_form_field(true),
            _ => return false,
        }
        true
    }

    /// Move to the next row on the form, down or up, past those not shown.
    fn move_form_field(&mut self, down: bool) {
        let form = &mut self.bench.form;
        let mut at = form.field.index();
        loop {
            let next = if down {
                (at + 1).min(FormField::ALL.len() - 1)
            } else {
                at.saturating_sub(1)
            };
            if next == at {
                return;
            }
            at = next;
            if form.shown(FormField::ALL[at]) {
                form.field = FormField::ALL[at];
                return;
            }
        }
    }

    fn change_form_value(&mut self, forward: bool) {
        let field = self.bench.form.field;
        if field.holds_model() {
            let mut choices = self.bench_model_choices();
            if field == FormField::Classifier {
                choices.insert(0, AUTO_CLASSIFIER.to_string());
            }
            if let Some(value) = self.bench.form.row_model_mut(field) {
                *value = cycle(&choices, value, forward);
            }
            self.bench.form.estimate = None;
            return;
        }
        match field {
            FormField::Work => {
                let count = self.bench.work_choices().len();
                let current = self.bench.work_index();
                let form = &mut self.bench.form;
                form.work = if forward {
                    (current + 1) % count
                } else {
                    (current + count - 1) % count
                };
                let most = self.bench.example_count();
                if most > 0 {
                    self.bench.form.tasks = self.bench.form.tasks.clamp(1, most);
                }
            }
            FormField::A | FormField::B => {
                self.settle_bench_against();
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
        if self.bench.form.b == DEFAULT_AGAINST {
            // `opus-5-5` itself matches only a Claude plan's id; with no
            // runnable Opus 5.5 (or no model list yet) the server's
            // strongest Opus stands in.
            self.bench.form.b = self.default_bench_against();
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
        let mut arms = [String::new(), String::new()];
        for (arm, value) in arms.iter_mut().enumerate() {
            match form.arm_value(arm) {
                Ok(arm_value) => *value = arm_value,
                Err(missing) => {
                    let difficulty = missing
                        .difficulty()
                        .map_or("", |(_, difficulty)| DIFFICULTIES[difficulty]);
                    self.bench.form.field = missing;
                    self.notification = Some((
                        format!(
                            "Pick a model for {difficulty} work of arm {}",
                            if arm == 0 { "A" } else { "B" }
                        ),
                        NotificationType::Warning,
                    ));
                    return;
                }
            }
        }
        let [a, b] = arms;
        let spec = BenchSpec {
            work,
            a,
            b,
            repeats: form.repeats,
            check: Some(form.check.clone()).filter(|c| !c.is_empty()),
            budget_usd,
            classifier: form.classifier_model(),
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
                unpriced_calls: 0,
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
    fn a_benchmark_run_opens_on_its_own_tab() {
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

    fn example(suite: &str, title: &str) -> BenchExample {
        BenchExample {
            suite: suite.to_string(),
            title: title.to_string(),
        }
    }

    #[test]
    fn default_form_runs_each_task_once_against_opus_5_5() {
        let form = BenchForm::default();
        assert_eq!(form.repeats, DEFAULT_REPEATS);
        assert_eq!(form.a, "gate");
        assert_eq!(form.b, DEFAULT_AGAINST);
        assert!(form.b_models.iter().all(String::is_empty));
        assert_eq!(form.arm_value(1), Ok(DEFAULT_AGAINST.to_string()));
        assert_eq!(BenchState::default().work(), WorkChoice::LastPrompt);
    }

    #[test]
    fn sessions_select_the_full_default_suite_regardless_of_its_position() {
        let mut app = App::new();
        let id = app.active_agent_id.clone();
        let examples = vec![
            example("coding", "easy 1"),
            example(DEFAULT_SUITE, "hard 1"),
            example("gate_hard", "other 1"),
            example(DEFAULT_SUITE, "hard 2"),
        ];
        app.cmd_bench_sessions(&id, Vec::new(), examples);
        assert_eq!(app.bench.work(), WorkChoice::Examples(Some(DEFAULT_SUITE)));
        assert_eq!(app.bench.form.tasks, 2);
    }

    #[test]
    fn default_submission_prepares_the_full_suite_against_opus_5_5() {
        let mut app = app_with_models(&[
            ("gate", "Gate"),
            ("openrouter/anthropic/claude-opus-5.5", "Claude Opus 5.5"),
            (OPUS_5_5, "Claude Opus 5.5"),
            ("claude/claude-sonnet-5-5", "Claude Sonnet 5.5"),
        ]);
        let id = app.active_agent_id.clone();
        let examples = vec![
            example("coding", "easy 1"),
            example(DEFAULT_SUITE, "hard 1"),
            example(DEFAULT_SUITE, "hard 2"),
        ];
        app.cmd_bench_sessions(&id, Vec::new(), examples);
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        app.event_tx = Some(tx);

        app.submit_bench_form();
        let event = rx.try_recv().expect("prepare request");
        let UiEvent::Bench {
            request: BenchRequest::Prepare(spec),
            ..
        } = event
        else {
            panic!("expected a benchmark prepare request");
        };
        assert_eq!(
            spec.work,
            BenchWork::Examples {
                suite: Some(DEFAULT_SUITE.to_string()),
                tasks: 2,
            }
        );
        assert_eq!(spec.a, "gate");
        assert_eq!(spec.b, OPUS_5_5, "the Claude plan's Opus 5.5");
        assert_eq!(spec.repeats, 1);
    }

    const OPUS_5_5: &str = "claude/claude-opus-5-5";

    #[test]
    fn opus_5_5_is_found_on_any_route_but_not_as_a_variant() {
        for id in [
            OPUS_5_5,
            "aura-claude-opus-5-5",
            "openrouter/anthropic/claude-opus-5.5",
        ] {
            assert!(is_opus_5_5(id), "{id}");
        }
        for id in [
            "claude/claude-opus-5-5-fast",
            "claude/claude-opus-4-7",
            "claude/claude-sonnet-5-5",
            "openrouter/anthropic/claude-opus-5.55",
        ] {
            assert!(!is_opus_5_5(id), "{id}");
        }

        let openrouter = "openrouter/anthropic/claude-opus-5.5";
        let app = app_with_models(&[("gate", "Gate"), (openrouter, "Claude Opus 5.5")]);
        assert_eq!(app.default_bench_against(), openrouter, "without a plan");
        let none = app_with_models(&[("claude/claude-opus-4-7", "Claude Opus 4.7")]);
        assert_eq!(
            none.default_bench_against(),
            "",
            "the server's strongest Opus stands in"
        );
    }

    #[test]
    fn arm_b_starts_on_the_runnable_opus_5_5_and_cycles_from_it() {
        let mut app = app_with_models(&[
            ("gate", "Gate"),
            ("claude/claude-sonnet-5-5", "Claude Sonnet 5.5"),
            (OPUS_5_5, "Claude Opus 5.5"),
            ("chatgpt/gpt-5.4", "GPT-5.4"),
        ]);
        app.open_bench_form();
        assert_eq!(app.bench.form.b, OPUS_5_5);
        app.bench.form.field = FormField::B;
        app.change_form_value(true);
        assert_eq!(app.bench.form.b, "chatgpt/gpt-5.4");
        app.change_form_value(false);
        assert_eq!(app.bench.form.b, OPUS_5_5, "and back to the default");
        app.change_form_value(false);
        assert_eq!(app.bench.form.b, "claude/claude-sonnet-5-5");
    }

    #[test]
    fn with_no_model_list_the_default_arm_b_is_sent_as_the_strongest_opus() {
        let mut app = App::new();
        app.models.clear();
        app.open_bench_form();
        assert_eq!(app.bench.form.b, DEFAULT_AGAINST, "kept until models load");
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        app.event_tx = Some(tx);
        app.submit_bench_form();
        let sent = std::iter::from_fn(|| rx.try_recv().ok()).find_map(|event| match event {
            UiEvent::Bench {
                request: BenchRequest::Prepare(spec),
                ..
            } => Some(spec.b),
            _ => None,
        });
        assert_eq!(sent.as_deref(), Some(""));
    }

    #[test]
    fn the_default_suite_is_explicit_even_when_it_is_the_only_suite() {
        let mut app = App::new();
        let id = app.active_agent_id.clone();
        app.cmd_bench_sessions(
            &id,
            Vec::new(),
            vec![example(DEFAULT_SUITE, "one"), example(DEFAULT_SUITE, "two")],
        );
        assert_eq!(app.bench.work(), WorkChoice::Examples(Some(DEFAULT_SUITE)));
        assert_eq!(app.bench.form.tasks, 2);
        assert!(app
            .bench
            .work_choices()
            .contains(&WorkChoice::Examples(Some(DEFAULT_SUITE))));
    }

    #[test]
    fn absent_default_suite_falls_back_to_all_examples() {
        let mut app = App::new();
        let id = app.active_agent_id.clone();
        app.cmd_bench_sessions(
            &id,
            Vec::new(),
            vec![example("coding", "one"), example("gate_hard", "two")],
        );
        assert_eq!(app.bench.work(), WorkChoice::Examples(None));
        assert_eq!(app.bench.form.tasks, 2);

        let mut empty = App::new();
        let id = empty.active_agent_id.clone();
        empty.cmd_bench_sessions(&id, Vec::new(), Vec::new());
        assert_eq!(empty.bench.work(), WorkChoice::LastPrompt);
    }

    #[test]
    fn refreshing_examples_preserves_a_user_chosen_work_selection() {
        let mut app = App::new();
        let id = app.active_agent_id.clone();
        let examples = vec![
            example("coding", "easy 1"),
            example(DEFAULT_SUITE, "hard 1"),
            example(DEFAULT_SUITE, "hard 2"),
        ];
        app.cmd_bench_sessions(&id, Vec::new(), examples.clone());
        app.bench.form.field = FormField::Work;
        app.change_form_value(true);
        assert_eq!(app.bench.work(), WorkChoice::LastPrompt);
        app.bench.form.tasks = 1;

        app.cmd_bench_sessions(&id, Vec::new(), examples);
        assert_eq!(app.bench.work(), WorkChoice::LastPrompt);
        assert_eq!(app.bench.form.tasks, 1);
    }

    #[test]
    fn successful_submission_resets_to_the_full_cached_suite() {
        let mut app = App::new();
        let id = app.active_agent_id.clone();
        app.cmd_bench_sessions(
            &id,
            Vec::new(),
            vec![
                example("coding", "easy 1"),
                example(DEFAULT_SUITE, "hard 1"),
                example(DEFAULT_SUITE, "hard 2"),
            ],
        );
        app.bench.mode = BenchMode::Form;
        app.bench.form.work = 1;
        app.bench.form.repeats = 3;

        app.cmd_bench_notice(Ok("started".to_string()));
        assert_eq!(app.bench.work(), WorkChoice::Examples(Some(DEFAULT_SUITE)));
        assert_eq!(app.bench.form.tasks, 2);
        assert_eq!(app.bench.form.repeats, DEFAULT_REPEATS);
        assert_eq!(app.bench.form.b, DEFAULT_AGAINST);
    }

    #[test]
    fn the_tasks_row_counts_from_one_to_the_example_tasks() {
        let mut app = App::new();
        let id = app.active_agent_id.clone();
        let titles: Vec<BenchExample> = (1..=7)
            .map(|i| example("coding", &format!("task {i}")))
            .collect();
        app.cmd_bench_sessions(&id, Vec::new(), titles);
        assert_eq!(app.bench.form.tasks, 7);
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

    /// An app offering `ids`, each available, from the provider its prefix
    /// names.
    fn app_with_models(ids: &[(&str, &str)]) -> App {
        let mut app = App::new();
        app.models = ids
            .iter()
            .map(|(id, label)| crate::events::ModelOptionView {
                id: (*id).to_string(),
                label: (*label).to_string(),
                local: false,
                installed: true,
                group: id.split('/').next().unwrap_or_default().to_string(),
                provider: None,
                available: true,
                context_window: None,
            })
            .collect();
        app
    }

    const DEEPSEEK: &str = "openrouter/deepseek/deepseek-v3.2";

    fn three_models() -> App {
        app_with_models(&[
            ("gate", "Gate"),
            ("claude/claude-sonnet-4-6", "Claude Sonnet 4.6"),
            (DEEPSEEK, "DeepSeek V3.2"),
            ("chatgpt/gpt-5.4", "GPT-5.4"),
        ])
    }

    #[test]
    fn difficulty_rows_show_only_for_a_by_difficulty_arm() {
        let mut app = three_models();
        app.bench.form.b.clear();
        assert!(app
            .bench_arm_choices(true)
            .contains(&BY_DIFFICULTY.to_string()));
        app.bench.form.field = FormField::B;
        app.move_form_field(true);
        assert_eq!(
            app.bench.form.field,
            FormField::Classifier,
            "skips hidden rows"
        );

        app.bench.form.b = BY_DIFFICULTY.to_string();
        app.bench.form.field = FormField::B;
        app.move_form_field(true);
        assert_eq!(app.bench.form.field, FormField::BEasy);
        app.bench.form.field = FormField::Classifier;
        app.move_form_field(false);
        assert_eq!(app.bench.form.field, FormField::BHard);
        assert!(!app.bench.form.shown(FormField::AEasy));
    }

    #[test]
    fn a_difficulty_row_cycles_models_and_enter_opens_the_model_selector() {
        let mut app = three_models();
        let session = app.current_model.clone();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        app.event_tx = Some(tx);
        app.bench.mode = BenchMode::Form;
        app.bench.form.a = BY_DIFFICULTY.to_string();
        app.bench.form.field = FormField::AEasy;
        app.change_form_value(true);
        assert_eq!(app.bench.form.a_models[0], "claude/claude-sonnet-4-6");
        app.change_form_value(false);
        assert_eq!(
            app.bench.form.a_models[0], "chatgpt/gpt-5.4",
            "only models, no Gate"
        );

        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert!(app.bench_form_key(key(KeyCode::Enter)));
        assert_eq!(app.state, AppState::SelectingModel);
        assert_eq!(
            app.model_picker.target(),
            PickerTarget::BenchDifficulty {
                arm: 0,
                difficulty: 0
            }
        );
        assert_eq!(app.picker_marked(), "chatgpt/gpt-5.4", "the row's model");
        for c in "deep".chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.state, AppState::Idle);
        assert_eq!(app.bench.form.a_models[0], DEEPSEEK);
        assert_eq!(app.bench.form.field, FormField::AMedium, "on to the next");
        assert_eq!(
            app.current_model, session,
            "the session's model is untouched"
        );
        assert!(
            !std::iter::from_fn(|| rx.try_recv().ok())
                .any(|event| matches!(event, UiEvent::SelectModel(_))),
            "nothing selects a model for the session"
        );
    }

    #[test]
    fn typing_on_a_difficulty_row_opens_the_selector_filtered() {
        let mut app = three_models();
        app.bench.mode = BenchMode::Form;
        app.bench.form.b = BY_DIFFICULTY.to_string();
        app.bench.form.field = FormField::BHard;
        app.input = "gpt".to_string();
        assert!(app.bench_fill_field());
        assert_eq!(app.state, AppState::SelectingModel);
        assert_eq!(app.model_picker.filter(), "gpt");
        assert!(app.input.is_empty());
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.bench.form.b_models[2], "chatgpt/gpt-5.4");
    }

    #[test]
    fn the_selector_refuses_a_model_that_needs_a_sign_in_for_a_row() {
        let mut app = three_models();
        app.models.push(crate::events::ModelOptionView {
            id: "openrouter/qwen/qwen3-coder".to_string(),
            label: "Qwen3 Coder".to_string(),
            local: false,
            installed: true,
            group: "openrouter".to_string(),
            provider: Some("openrouter".to_string()),
            available: false,
            context_window: None,
        });
        app.bench.mode = BenchMode::Form;
        app.bench.form.a = BY_DIFFICULTY.to_string();
        app.open_bench_model_picker(FormField::AMedium, "");
        app.pick_bench_model(FormField::AMedium, "openrouter/qwen/qwen3-coder");
        assert_eq!(app.state, AppState::SelectingModel, "stays open");
        assert!(app.bench.form.a_models[1].is_empty());
        assert!(app.notification.as_ref().is_some_and(|(text, _)| {
            text == "Qwen3 Coder needs a sign-in first: /login openrouter"
        }));

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.state, AppState::Idle, "Esc leaves the row as it was");
        assert!(app.bench.form.a_models[1].is_empty());
    }

    #[test]
    fn a_by_difficulty_arm_needs_all_three_models_and_is_sent_as_one_string() {
        let mut app = three_models();
        app.bench.form.b = BY_DIFFICULTY.to_string();
        app.bench.form.b_models = [DEEPSEEK.to_string(), String::new(), String::new()];
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        app.event_tx = Some(tx);
        app.bench.form.field = FormField::Go;
        app.submit_bench_form();
        assert_eq!(app.bench.form.field, FormField::BMedium);
        assert!(app
            .notification
            .as_ref()
            .is_some_and(|(text, _)| text == "Pick a model for medium work of arm B"));

        app.bench.form.b_models[1] = "claude/claude-sonnet-4-6".to_string();
        app.bench.form.b_models[2] = "chatgpt/gpt-5.4".to_string();
        app.submit_bench_form();
        let sent = std::iter::from_fn(|| rx.try_recv().ok()).find_map(|event| match event {
            UiEvent::Bench {
                request: BenchRequest::Prepare(spec),
                ..
            } => Some(spec.b),
            _ => None,
        });
        assert_eq!(
            sent.as_deref(),
            Some(
                "difficulty:easy=openrouter/deepseek/deepseek-v3.2,\
                 medium=claude/claude-sonnet-4-6,hard=chatgpt/gpt-5.4"
            )
        );
    }

    #[test]
    fn the_classifier_row_starts_on_haiku_cycles_models_and_is_sent() {
        let mut app = three_models();
        app.bench.form.b.clear();
        app.bench.mode = BenchMode::Form;
        app.bench.form.field = FormField::B;
        app.move_form_field(true);
        assert_eq!(
            app.bench.form.field,
            FormField::Classifier,
            "after the arms"
        );
        assert_eq!(app.bench.form.classifier, DEFAULT_CLASSIFIER);
        assert_eq!(
            app.bench.form.classifier_model(),
            Some(DEFAULT_CLASSIFIER.to_string()),
            "haiku is sent for the server to find"
        );

        app.change_form_value(true);
        assert_eq!(
            app.bench.form.classifier, AUTO_CLASSIFIER,
            "auto comes first"
        );
        assert_eq!(app.bench.form.classifier_model(), None);
        app.change_form_value(true);
        assert_eq!(app.bench.form.classifier, "claude/claude-sonnet-4-6");
        app.change_form_value(false);
        assert_eq!(app.bench.form.classifier, AUTO_CLASSIFIER);
        app.change_form_value(false);
        assert_eq!(app.bench.form.classifier, "chatgpt/gpt-5.4", "and wraps");

        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        app.event_tx = Some(tx);
        app.bench.form.field = FormField::Go;
        app.submit_bench_form();
        let sent = std::iter::from_fn(|| rx.try_recv().ok()).find_map(|event| match event {
            UiEvent::Bench {
                request: BenchRequest::Prepare(spec),
                ..
            } => Some(spec.classifier),
            _ => None,
        });
        assert_eq!(sent, Some(Some("chatgpt/gpt-5.4".to_string())));
    }

    #[test]
    fn enter_on_the_classifier_row_picks_it_in_the_selector_and_backspace_resets_it() {
        let mut app = three_models();
        app.bench.mode = BenchMode::Form;
        app.bench.form.field = FormField::Classifier;
        assert!(app.bench_form_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        assert_eq!(app.state, AppState::SelectingModel);
        assert_eq!(app.model_picker.target(), PickerTarget::BenchClassifier);

        app.pick_bench_model(FormField::Classifier, DEEPSEEK);
        assert_eq!(app.state, AppState::Idle);
        assert_eq!(app.bench.form.classifier, DEEPSEEK);
        assert_eq!(app.bench.form.field, FormField::Repeats, "on to the next");
        assert_eq!(
            app.current_model,
            App::new().current_model,
            "session unchanged"
        );

        app.bench.form.field = FormField::Classifier;
        assert!(app.bench_form_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)));
        assert_eq!(app.bench.form.classifier, DEFAULT_CLASSIFIER);
    }

    #[test]
    fn opening_the_form_names_the_runnable_haiku_and_keeps_a_chosen_classifier() {
        const HAIKU: &str = "claude/claude-haiku-4-5";
        let mut app = app_with_models(&[
            ("gate", "Gate"),
            ("claude/claude-sonnet-4-6", "Claude Sonnet 4.6"),
            (HAIKU, "Claude Haiku 4.5"),
        ]);
        app.open_bench_form();
        assert_eq!(app.bench.form.classifier, HAIKU);
        assert_eq!(app.bench.form.classifier_model(), Some(HAIKU.to_string()));

        app.bench.form.classifier = AUTO_CLASSIFIER.to_string();
        app.open_bench_form();
        assert_eq!(
            app.bench.form.classifier, AUTO_CLASSIFIER,
            "a classifier the user chose stays"
        );

        app.bench.form.field = FormField::Classifier;
        assert!(app.bench_form_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)));
        assert_eq!(
            app.bench.form.classifier, HAIKU,
            "backspace goes back to Haiku"
        );
    }
}
