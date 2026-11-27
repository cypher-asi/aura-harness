//! The BENCH form that sets a new benchmark up: its rows, what each
//! cycles through, and what Enter on its last row sends. Enter on a
//! difficulty's row opens the `/model` selector to pick its model.
//!
//! Its first row picks the top-level agent the benchmark is run from: the
//! runs work on copies of its workspace, and can replay its last prompt or
//! one of its sessions. It starts on the agent at the top of the tree last
//! on screen.

mod choices;
mod fields;
mod keys;
mod submit;

pub use fields::FormField;

use super::{BenchMode, BenchState};
use crate::app::{App, Dialog, NotificationType};
use crate::events::{BenchExample, BenchRequest, BenchSession};
use ratatui::layout::Rect;
#[cfg(test)]
use {
    crate::app::PickerTarget,
    crate::events::{AgentSummary, BenchWork},
    crossterm::event::{KeyCode, KeyEvent},
};

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
    /// Hex id of the agent it is run from; `None` until the form opens
    /// with a top-level agent to offer
    pub from: Option<String>,
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
            from: None,
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

impl BenchState {
    /// Where the form was drawn this frame (`area`) and the screen row of
    /// each of its rows; `None` when it is not shown.
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
    /// Where the BENCH form was drawn this frame and the screen row of
    /// each of its rows, for clicks; `None` when it is not shown.
    pub(crate) fn set_bench_form_rows(&mut self, area: Option<Rect>, rows: Vec<(u16, FormField)>) {
        self.bench.set_form_rows(area, rows);
    }

    /// Open the model selector, as `/model` does, to pick the model of the
    /// form's row `field` (a difficulty's, or the classifier's), filtered
    /// by `filter` and on the row's model when it has one.
    pub(in crate::app) fn open_bench_model_picker(&mut self, field: FormField, filter: &str) {
        let Some(target) = field.picker_target() else {
            return;
        };
        if self.account.models.is_empty() {
            self.notifications.current = Some((
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
        self.account
            .model_picker
            .open_for(&self.account.models, &current, target, filter);
        self.open_dialog(Dialog::ModelSelector);
    }

    /// Put the model `id` picked in the selector on the form's row
    /// `field`, and go on to the next row. A model that can't run yet is
    /// refused with what it needs, and the selector stays open.
    pub(in crate::app) fn pick_bench_model(&mut self, field: FormField, id: &str) {
        let needs = self
            .account
            .models
            .iter()
            .find(|m| m.id == id)
            .and_then(|m| {
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
            self.notifications.current = Some((needs, NotificationType::Warning));
            return;
        }
        let form = &mut self.bench.form;
        let Some(model) = form.row_model_mut(field) else {
            return;
        };
        *model = id.to_string();
        form.estimate = None;
        form.field = field;
        self.close_dialog();
        self.move_form_field(true);
    }

    /// The model the selector marks as chosen: the session's, or the
    /// benchmark row's it was opened for.
    #[must_use]
    pub fn picker_marked(&self) -> &str {
        match FormField::of_picker(self.account.model_picker.target()) {
            None => &self.account.current_model,
            Some(field) => self.bench.form.row_model(field).unwrap_or_default(),
        }
    }

    pub(in crate::app) fn cmd_bench_sessions(
        &mut self,
        agent_id: &str,
        sessions: Vec<BenchSession>,
        examples: Vec<BenchExample>,
    ) {
        if agent_id == self.bench_form_agent() {
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

    pub(in crate::app) fn cmd_bench_estimate(
        &mut self,
        agent_id: &str,
        estimate: Result<String, String>,
    ) {
        if agent_id == self.bench_form_agent() {
            self.bench.form.waiting = false;
            if estimate.is_ok() {
                self.bench.form.field = FormField::Go;
            }
            self.bench.form.estimate = Some(estimate);
        }
    }

    /// Enter with text in the input line while the form's focused row is
    /// typed: the text becomes its value. Returns whether it did.
    pub(in crate::app) fn bench_fill_field(&mut self) -> bool {
        let field = self.bench.form.field;
        if self.bench.mode != Some(BenchMode::Form)
            || !field.is_text()
            || self.input.text.is_empty()
        {
            return false;
        }
        let text = std::mem::take(&mut self.input.text);
        self.input.cursor_pos = 0;
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

    /// Open the form, asking for the sessions it offers. Dropping it shows
    /// what is shown now again.
    pub(in crate::app) fn open_bench_form(&mut self) {
        self.bench.before_form = self
            .bench
            .mode
            .replace(BenchMode::Form)
            .filter(|was| *was != BenchMode::Form);
        self.settle_bench_from();
        if self.bench.form.classifier == DEFAULT_CLASSIFIER {
            // Name the Haiku the user runs, now the model list is likely
            // in; a classifier the user chose is left alone.
            self.bench.form.classifier = self.default_bench_classifier();
        }
        self.settle_bench_against();
        self.bench.form.estimate = None;
        self.bench.form.waiting = false;
        self.send_form_bench(BenchRequest::Sessions);
    }

    /// A left click on BENCH's chat panel: on a row of the form it goes to
    /// the row, opens the model selector on a difficulty's row, and on the
    /// last row asks for the estimate or starts the benchmark, as Enter
    /// does there. Returns whether it hit a row.
    pub(crate) fn click_bench(&mut self, column: u16, row: u16) -> bool {
        if self.bench.mode != Some(BenchMode::Form) {
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
}

#[cfg(test)]
mod tests;
