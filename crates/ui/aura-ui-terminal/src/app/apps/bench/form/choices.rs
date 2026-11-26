//! What the form's rows offer: the work, the agents and the models to
//! pick from, and the defaults the form starts on.

use super::{
    is_opus_5_5, BenchForm, WorkChoice, BY_DIFFICULTY, DEFAULT_AGAINST, DEFAULT_CLASSIFIER,
    DEFAULT_SUITE, DEFAULT_WORK,
};
use crate::app::apps::bench::BenchState;
use crate::app::App;
use crate::events::{AgentSummary, BenchExample, BenchRequest};

impl BenchState {
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
    pub(in crate::app::apps::bench) fn reset_form(&mut self) {
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
    pub(super) fn work_index(&self) -> usize {
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
    pub(super) fn clamp_tasks(&mut self) {
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
}

impl App {
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
        self.account
            .models
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
    pub(super) fn settle_bench_against(&mut self) {
        if self.bench.form.b == DEFAULT_AGAINST && !self.account.models.is_empty() {
            self.bench.form.b = self.default_bench_against();
        }
    }

    /// What the form's From row cycles through: the listed top-level
    /// agents, but a benchmark's own runs.
    #[must_use]
    pub fn bench_from_choices(&self) -> Vec<&AgentSummary> {
        self.agents
            .iter()
            .filter(|a| a.depth == 0 && !a.bench_run && a.archived_at_ms.is_none())
            .collect()
    }

    /// The agent the From row starts on: the one at the top of the tree
    /// last on screen, else the first it offers.
    fn default_bench_from(&self) -> Option<String> {
        let choices = self.bench_from_choices();
        let root = self.bench_tree_root();
        choices
            .iter()
            .find(|a| Some(a.id.as_str()) == root)
            .or_else(|| choices.first())
            .map(|a| a.id.clone())
    }

    /// Keep the From row on an agent it still offers, else put it on its
    /// default.
    pub(super) fn settle_bench_from(&mut self) {
        let offered = |id: &str| self.bench_from_choices().iter().any(|a| a.id == id);
        if self.bench.form.from.as_deref().is_some_and(offered) {
            return;
        }
        let from = self.default_bench_from();
        if from != self.bench.form.from {
            self.set_bench_from(from);
        }
    }

    /// Run the benchmark from `from`. The sessions offered were another
    /// agent's, so a session picked as the work gives way to the default.
    pub(super) fn set_bench_from(&mut self, from: Option<String>) {
        let session = matches!(self.bench.work(), WorkChoice::Session(_));
        self.bench.sessions.clear();
        let form = &mut self.bench.form;
        form.from = from;
        if session {
            form.work = DEFAULT_WORK;
        }
        form.estimate = None;
        form.waiting = false;
    }

    /// The agent the form's requests name: the one it is run from, else
    /// the one on screen.
    pub(super) fn bench_form_agent(&self) -> &str {
        self.bench
            .form
            .from
            .as_deref()
            .unwrap_or(&self.active_agent_id)
    }

    pub(super) fn send_form_bench(&self, request: BenchRequest) {
        self.send_bench_for(self.bench_form_agent(), request);
    }

    /// What the From row shows: the agent's name, and its project's.
    #[must_use]
    pub fn bench_from_label(&self) -> Option<String> {
        let from = self.bench.form.from.as_deref()?;
        let agent = self.agents.iter().find(|a| a.id == from)?;
        Some(match &agent.project {
            Some(project) => format!("{} · {project}", agent.name),
            None => agent.name.clone(),
        })
    }
}
