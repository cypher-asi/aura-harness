//! `/compare [prompt]`: the comparison a request asks for, from the work
//! it runs and the arms it runs on, and what it should cost.

use super::kernel_of;
use super::run::ComparePlan;
use super::view::{arm_label, plural, EstimateView};
use crate::bench_pack::{self, ExampleTask};
use crate::event_loop::bench_source::{self, Replay, StartFile};
use crate::event_loop::LoopState;
use aura_agent_kernel::Kernel;
use aura_context_prompts::model_messages::build_plan::build_plan_prompt;
use aura_core_types::{AgentId, ArmSpec, BenchSource, Difficulty, PromptCategory, RouteLevel};
use aura_gate::compare::{default_against, estimate, resolve_arms, CompareRequest};
use aura_gate::route::Candidate;
use aura_model::model_catalog::route_label;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Prepare the comparison `request` asks for from `agent_id`'s chat and
/// say what it should cost.
pub(crate) async fn prepare(
    state: &mut LoopState<'_>,
    agent_id: AgentId,
    request: CompareRequest,
) -> Result<String, String> {
    if let Some(running) = state.compare.running() {
        return Err(format!(
            "{} is still running; /compare cancel stops it",
            running.name
        ));
    }
    let kernel = kernel_of(state, agent_id)?;
    let workspace = kernel.agent_workspace();
    let workspace = workspace.canonicalize().unwrap_or(workspace);
    let work = Work::of(state, agent_id, &request, &kernel, &workspace)?;
    let arms = Arms::of(state, &request)?;
    let category = work.category;
    let plan = plan_of(state, agent_id, request, work, arms, workspace);
    let text = estimate_of(state, &plan, category);
    state.compare.pending = Some(plan);
    Ok(text)
}

/// What a comparison's runs work on, and where it comes from.
struct Work {
    /// The prompts each run sends in order; none for example tasks
    prompts: Vec<String>,
    examples: Vec<&'static ExampleTask>,
    source: BenchSource,
    /// Files put in each copy before its run
    start: Vec<StartFile>,
    /// What setting it up found out, for the record
    notes: Vec<String>,
    /// What the last turn asked for, when its prompt is the work
    category: Option<PromptCategory>,
}

impl Work {
    /// Work from `source` with nothing in it yet.
    fn new(source: BenchSource) -> Self {
        Self {
            prompts: Vec::new(),
            examples: Vec::new(),
            source,
            start: Vec::new(),
            notes: Vec::new(),
            category: None,
        }
    }

    /// The work `request` asks for: example tasks, a replayed session, a
    /// plan's build prompt, the prompt given, or else the prompt of
    /// `agent_id`'s last turn on `kernel`.
    fn of(
        state: &LoopState<'_>,
        agent_id: AgentId,
        request: &CompareRequest,
        kernel: &Kernel,
        workspace: &Path,
    ) -> Result<Self, String> {
        if request.examples.is_some() || request.suite.is_some() {
            let examples = example_tasks(request.examples, request.suite.as_deref())?;
            let tasks = examples.iter().map(|task| task.info()).collect();
            return Ok(Self {
                examples,
                ..Self::new(BenchSource::Examples { tasks })
            });
        }
        match (request.session, &request.plan, &request.prompt) {
            (Some(pick), _, _) => {
                let Replay {
                    prompts,
                    source,
                    start,
                    notes,
                } = bench_source::load(state, agent_id, pick, workspace)?;
                Ok(Self {
                    prompts,
                    start,
                    notes,
                    ..Self::new(source)
                })
            }
            (None, Some(plan), _) if !workspace.join(plan).is_file() => {
                Err(format!("No plan at {plan} in {}", workspace.display()))
            }
            (None, Some(plan), _) => Ok(Self::prompt(build_plan_prompt(plan))),
            (None, None, Some(given)) => Ok(Self::prompt(given.clone())),
            (None, None, None) => Self::last_turn(kernel, agent_id),
        }
    }

    /// The one prompt `text`.
    fn prompt(text: String) -> Self {
        Self {
            prompts: vec![text],
            ..Self::new(BenchSource::Prompt)
        }
    }

    /// The prompt of `agent_id`'s last turn on `kernel`, with what it
    /// asked for.
    fn last_turn(kernel: &Kernel, agent_id: AgentId) -> Result<Self, String> {
        let no_prompt =
            || "No prompt to compare yet: give one, e.g. /compare fix the failing test".to_string();
        let last =
            aura_gate::live::last_turn(kernel.store().as_ref(), agent_id).ok_or_else(no_prompt)?;
        let category = Some(last.outcome.category);
        let text = last.prompt.ok_or_else(no_prompt)?.text;
        Ok(Self {
            category,
            ..Self::prompt(text)
        })
    }

    /// What the comparison is about: its first prompt, else how many
    /// example tasks it runs.
    fn task(&self) -> String {
        match self.prompts.first() {
            Some(first) => first.clone(),
            None => format!(
                "{} example coding task{}",
                self.examples.len(),
                plural(self.examples.len())
            ),
        }
    }

    /// The command that checks each run: `given`, else the one set for
    /// edits in `workspace`; none for example tasks, which bring theirs.
    fn check(&self, given: Option<String>, workspace: &Path) -> Option<String> {
        if !self.examples.is_empty() {
            return None;
        }
        given.or_else(|| {
            crate::harness::post_edit_check_from_env()
                .resolve(workspace)
                .map(|check| check.command)
        })
    }
}

/// The first `n` built-in example tasks of `suite` (of every suite when
/// `None`); all of them when `n` is `None`.
fn example_tasks(
    n: Option<usize>,
    suite: Option<&str>,
) -> Result<Vec<&'static ExampleTask>, String> {
    let all = bench_pack::tasks();
    if all.is_empty() {
        return Err("No example tasks are built into this build".to_string());
    }
    let mut pool: Vec<&'static ExampleTask> = all
        .iter()
        .filter(|task| suite.is_none_or(|suite| task.suite() == suite))
        .collect();
    if pool.is_empty() {
        return Err(format!(
            "No suite `{}`; the suites are {}",
            suite.unwrap_or_default(),
            bench_pack::suites().join(", ")
        ));
    }
    let n = n.unwrap_or(pool.len());
    if n == 0 || n > pool.len() {
        return Err(format!(
            "There are {} example tasks{}; ask for 1 to {}",
            pool.len(),
            suite.map_or_else(String::new, |suite| format!(" in {suite}")),
            pool.len()
        ));
    }
    pool.truncate(n);
    Ok(pool)
}

/// What a comparison runs on and which model judges it.
struct Arms {
    /// What arm A and arm B run on
    specs: [ArmSpec; 2],
    judge: String,
    /// What Gate routes a Gate arm among
    candidates: Vec<Candidate>,
    /// Every model the user can run on any route
    runnable: Vec<Candidate>,
}

impl Arms {
    /// The arms `request` names, Gate against the strongest model when it
    /// names neither, judged by the strongest model, else by the model
    /// either arm runs hard work on.
    fn of(state: &LoopState<'_>, request: &CompareRequest) -> Result<Self, String> {
        let candidates = state.models.gate_candidates();
        if candidates.is_empty() {
            return Err(
                "Nothing to compare on yet: /login, link a plan, or start a local model".into(),
            );
        }
        let strongest = default_against(&candidates).map(|c| c.id.as_str());
        let a = request.a.as_deref().unwrap_or("gate");
        let b = match request.against.as_deref() {
            Some(query) => query,
            None if a.trim().to_ascii_lowercase().starts_with("gate") => {
                strongest.ok_or("No model to compare Gate with")?
            }
            None => "gate",
        };
        let runnable = state.models.runnable_models();
        let delegate = state.hub.gate.settings().delegate;
        let (arm_a, arm_b) = resolve_arms((a, b), &runnable, &candidates, delegate)?;
        let judge = match strongest {
            Some(model) => model.to_string(),
            None => [&arm_a, &arm_b]
                .into_iter()
                .find_map(|spec| spec.model_for(Some(Difficulty::Hard)))
                .unwrap_or_default()
                .to_string(),
        };
        Ok(Self {
            specs: [arm_a, arm_b],
            judge,
            candidates,
            runnable,
        })
    }
}

/// The comparison of `work` on `arms` in `workspace` that `user` asked
/// for, set up as `request` says.
fn plan_of(
    state: &LoopState<'_>,
    user: AgentId,
    request: CompareRequest,
    mut work: Work,
    arms: Arms,
    workspace: PathBuf,
) -> ComparePlan {
    if request.check.is_some() && !work.examples.is_empty() {
        let note = "Example tasks use their own check commands; --check is ignored";
        work.notes.push(note.to_string());
    }
    if request.learn {
        work.notes.push(format!(
            "Learn mode: Gate arms explore as routed turns do ({:.0}% of turns), so the runs \
             teach Gate about models it would not pick yet",
            state.hub.gate.settings().explore_rate * 100.0
        ));
    }
    ComparePlan {
        task: work.task(),
        check: work.check(request.check, &workspace),
        prompts: work.prompts,
        examples: work.examples,
        plan: request.plan,
        arms: arms.specs,
        source: work.source,
        start: Arc::new(work.start),
        notes: work.notes,
        judge: arms.judge,
        repeats: request.repeats,
        budget_usd: request.budget_usd,
        candidates: arms.candidates,
        runnable: arms.runnable,
        classifier: state.models.classifier_model(),
        workspace,
        user,
        root: state.compare.root.clone(),
        concurrency: state.compare.concurrency,
        learn: request.learn,
    }
}

/// What the user is asked to confirm before `plan` starts, with Gate's
/// likely pick for work asking for `category`.
fn estimate_of(
    state: &LoopState<'_>,
    plan: &ComparePlan,
    category: Option<PromptCategory>,
) -> String {
    let expected = expected_pick(state, &plan.candidates, category);
    let expected = expected.as_deref();
    let [a, b] = &plan.arms;
    // A by-difficulty arm is priced on its medium model, which work of
    // unknown difficulty runs on.
    let [a_model, b_model] =
        [a, b].map(|spec| spec.model_for(None).or(expected).unwrap_or(&plan.judge));
    let reads = |spec: &ArmSpec| match (spec, expected) {
        (ArmSpec::Gate { .. }, Some(model)) => {
            format!("{} (likely {})", arm_label(spec), route_label(model))
        }
        _ => arm_label(spec),
    };
    let session = match &plan.source {
        BenchSource::Session { title, .. } => Some(title.as_str()),
        BenchSource::Prompt | BenchSource::Examples { .. } => None,
    };
    let runs = plan.runs_per_arm();
    EstimateView {
        task: &plan.task,
        prompts: plan.prompts.len(),
        session,
        examples: plan.source.examples(),
        a: reads(a),
        b: reads(b),
        judge: &plan.judge,
        repeats: plan.repeats,
        check: plan.check.as_deref(),
        budget_usd: plan.budget_usd,
        concurrency: plan.concurrency,
        estimate: estimate(a_model, b_model, &plan.judge, runs, prompts_per_run(plan)),
        notes: &plan.notes,
    }
    .render()
}

/// The model Gate would likely route the work to: its pick for
/// `category` as it routes now, without exploring, else its fixed pick.
fn expected_pick(
    state: &LoopState<'_>,
    candidates: &[Candidate],
    category: Option<PromptCategory>,
) -> Option<String> {
    let Some(category) = category else {
        return aura_gate::route::fixed_pick(candidates).map(|c| c.id.clone());
    };
    let live = &state.hub.gate;
    let settings = live.settings().without_exploring();
    let work = aura_gate::route::Work::new(RouteLevel::Turn, category, None);
    live.with_stats(|stats| {
        aura_gate::route::pick(stats, &work, candidates, &settings, &mut || 1.0)
    })
    .map(|decision| decision.model.id)
}

/// Prompts each run sends: the plan's, or the example tasks' on average,
/// rounded up.
fn prompts_per_run(plan: &ComparePlan) -> usize {
    if plan.examples.is_empty() {
        return plan.prompts.len();
    }
    let prompts: usize = plan.examples.iter().map(|task| task.prompts.len()).sum();
    prompts.div_ceil(plan.examples.len())
}
