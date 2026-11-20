//! `/compare`: run the user's own task (a prompt, a plan, or a session
//! replayed prompt by prompt) on two arms side by side, each Gate or a
//! pinned model, then pick or apply the better run. The BENCH tab drives
//! the same actions (see [`super::bench`]).
//!
//! `/compare [prompt]` prepares a comparison and shows what it should
//! cost; `/compare go` starts it (see [`super::compare_run`]). Each
//! comparison is an agent (`Compare #n`) under the agent that asked,
//! with its runs under it; its state is the latest `compare` system
//! record on its own record, which is also where `/compare` lists past
//! comparisons from. Copies are removed by `/compare apply` and
//! `/compare dismiss`.

use super::agent_events::AgentUi;
use super::bench_source;
use super::compare_run::{self, patches, remove_copies, save, ComparePlan};
use super::compare_view::{self, arm_label, split_patch, EstimateView};
use super::handlers::system_message;
use super::subagents::lock;
use super::LoopState;
use crate::bench_pack::{self, ExampleTask};
use aura_agent_gate::compare::{
    default_against, estimate, parse, resolve_arms, CompareCommand, CompareRequest,
    DEFAULT_CONCURRENCY,
};
use aura_agent_kernel::Kernel;
use aura_config::GateConfig;
use aura_context_prompts::model_messages::build_plan::build_plan_prompt;
use aura_core_types::{
    AgentId, ArmSpec, BenchSource, CompareArm, ComparePayload, CompareStatus, OutcomeSignal,
    PromptCategory, RouteLevel,
};
use aura_model::model_catalog::{model_key, route_label};
use aura_store_db::ReadStore;
use aura_ui_terminal::UiCommand;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Entries scanned back from the head of a comparison's record for its
/// latest state.
const SCAN_LIMIT: usize = 2_000;

/// A comparison under way.
struct Running {
    host: AgentId,
    name: String,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

/// `/compare`'s state in the event loop: where copies go, the
/// comparison waiting for `/compare go`, and the one running.
pub(crate) struct CompareState {
    root: PathBuf,
    concurrency: usize,
    pending: Option<ComparePlan>,
    running: Option<Running>,
}

impl CompareState {
    /// Copies under `root`, at most `concurrency` runs at once.
    pub(crate) fn new(root: PathBuf, concurrency: usize) -> Self {
        Self {
            root,
            concurrency,
            pending: None,
            running: None,
        }
    }

    /// Copies under `AURA_HOME/compare`, else under `data_dir`.
    pub(crate) fn in_aura_home(data_dir: &std::path::Path) -> Self {
        let root = aura_config::AuraHome::resolve()
            .map(|home| home.path.join("compare"))
            .unwrap_or_else(|e| {
                warn!(error = %e, "AURA_HOME unresolved; /compare copies go in the data directory");
                data_dir.join("compare")
            });
        Self::new(root, DEFAULT_CONCURRENCY)
    }

    fn running(&self) -> Option<&Running> {
        self.running.as_ref().filter(|r| !r.task.is_finished())
    }

    /// Wait for the running comparison to end (for tests).
    #[cfg(test)]
    pub(super) async fn finished(&mut self) {
        if let Some(running) = self.running.take() {
            let _ = running.task.await;
        }
    }
}

/// `/compare <args>` from the chat of `agent_id`.
pub(super) async fn handle(state: &mut LoopState<'_>, agent_id: AgentId, args: &str) {
    let ui = AgentUi::new(state.commands.clone(), agent_id);
    let command = match parse(args) {
        Ok(command) => command,
        Err(problem) => {
            ui.send(UiCommand::ShowWarning(problem)).await;
            return;
        }
    };
    state.park_finished_spawns().await;
    let shown = match command {
        CompareCommand::List => Ok(list(state)),
        CompareCommand::Show(n) => show(state, n),
        CompareCommand::Start(request) => prepare(state, agent_id, request)
            .await
            .map(|text| format!("{text}\n  /compare go starts it · /compare cancel drops it")),
        CompareCommand::Go => start(state).await,
        CompareCommand::Cancel => Ok(cancel(state)),
        CompareCommand::Pick(arm) => pick(state, None, &arm).await,
        CompareCommand::Apply(run) => apply(state, None, &run).await,
        CompareCommand::Diff(run) => diff(state, &ui, None, run.as_deref()).await,
        CompareCommand::Dismiss => dismiss(state, None).await,
    };
    match shown {
        Ok(text) if text.is_empty() => {}
        Ok(text) => ui.send(system_message(text)).await,
        Err(problem) => ui.send(UiCommand::ShowWarning(problem)).await,
    }
}

/// The latest state of the comparison held by `host`, if recorded.
fn latest_payload(store: &dyn ReadStore, host: AgentId) -> Option<ComparePayload> {
    let head = store.get_head_seq(host).ok()?;
    store
        .scan_record_descending(host, head, SCAN_LIMIT)
        .ok()?
        .iter()
        .find_map(|entry| entry.tx.compare_payload())
}

/// Every comparison, oldest first, with its latest state.
pub(super) fn comparisons(state: &LoopState<'_>) -> Vec<(AgentId, String, Option<ComparePayload>)> {
    let hosts: Vec<(AgentId, String)> = lock(&state.hub.registry)
        .compares()
        .iter()
        .map(|agent| (agent.agent_id(), agent.name.clone()))
        .collect();
    let store = state.hub.factory.store.as_ref();
    hosts
        .into_iter()
        .map(|(id, name)| (id, name, latest_payload(store, id)))
        .collect()
}

fn list(state: &LoopState<'_>) -> String {
    let all: Vec<(String, Option<ComparePayload>)> = comparisons(state)
        .into_iter()
        .map(|(_, name, payload)| (name, payload))
        .collect();
    let mut text = compare_view::list(&all);
    if let Some(running) = state.compare.running() {
        text.push_str(&format!(
            "\n{} is running; /compare cancel stops it.",
            running.name
        ));
    }
    if state.compare.pending.is_some() {
        text.push_str("\nA comparison is waiting: /compare go starts it.");
    }
    text
}

fn show(state: &LoopState<'_>, n: Option<usize>) -> Result<String, String> {
    let all = comparisons(state);
    let chosen = match n {
        Some(n) => all.get(n - 1),
        None => all.iter().rev().find(|(_, _, payload)| payload.is_some()),
    };
    match chosen {
        Some((_, name, Some(payload))) => Ok(compare_view::render(name, payload)),
        Some((_, name, None)) => Err(format!("{name} has no record")),
        None => Err("No such comparison; /compare lists them".to_string()),
    }
}

/// The comparison `host` holds, else the latest, once it has ended, for
/// pick, apply, diff and dismiss.
fn ended(
    state: &LoopState<'_>,
    host: Option<AgentId>,
) -> Result<(AgentId, String, ComparePayload), String> {
    let all = comparisons(state).into_iter().rev();
    let (host, name, payload) = match host {
        Some(wanted) => all
            .filter(|(id, _, _)| *id == wanted)
            .find_map(|(id, name, payload)| Some((id, name, payload?)))
            .ok_or_else(|| "No such benchmark".to_string())?,
        None => all
            .filter_map(|(id, name, payload)| Some((id, name, payload?)))
            .next()
            .ok_or_else(|| "No comparison yet; /compare [prompt] runs one".to_string())?,
    };
    if state.compare.running().is_some_and(|r| r.host == host)
        || payload.status == CompareStatus::Running
    {
        return Err(format!("{name} is still running; wait for it to end"));
    }
    Ok((host, name, payload))
}

/// The kernel of `agent_id`, opening its session when nothing has.
pub(super) fn kernel_of(
    state: &mut LoopState<'_>,
    agent_id: AgentId,
) -> Result<Arc<Kernel>, String> {
    if let Some(kernel) = state.kernel_for(agent_id) {
        return Ok(kernel);
    }
    let session = super::agents::open_session(state, agent_id)?;
    let kernel = Arc::clone(&session.kernel);
    state.idle.insert(agent_id, session);
    Ok(kernel)
}

/// The first `n` built-in example tasks.
fn example_tasks(n: usize) -> Result<Vec<&'static ExampleTask>, String> {
    let all = bench_pack::tasks();
    if all.is_empty() {
        return Err("No example tasks are built into this build".to_string());
    }
    if n == 0 || n > all.len() {
        return Err(format!(
            "There are {} example tasks; ask for 1 to {}",
            all.len(),
            all.len()
        ));
    }
    Ok(all[..n].iter().collect())
}

/// Prepare the comparison `request` asks for from `agent_id`'s chat and
/// say what it should cost.
pub(super) async fn prepare(
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
    let last = super::gate_record::last_turn(kernel.store().as_ref(), agent_id);
    let mut category = PromptCategory::Unclassified;
    let mut replay = None;
    let examples = match request.examples {
        Some(n) => example_tasks(n)?,
        None => Vec::new(),
    };
    let prompts = match (request.session, &request.plan, &request.prompt) {
        _ if !examples.is_empty() => Vec::new(),
        (Some(pick), _, _) => {
            let loaded = bench_source::load(state, agent_id, pick, &workspace)?;
            let prompts = loaded.prompts.clone();
            replay = Some(loaded);
            prompts
        }
        (None, Some(plan), _) => {
            if !workspace.join(plan).is_file() {
                return Err(format!("No plan at {plan} in {}", workspace.display()));
            }
            vec![build_plan_prompt(plan)]
        }
        (None, None, Some(prompt)) => vec![prompt.clone()],
        (None, None, None) => {
            let prompt = last
                .and_then(|turn| {
                    category = turn.outcome.category;
                    turn.prompt
                })
                .ok_or_else(|| {
                    "No prompt to compare yet: give one, e.g. /compare fix the failing test"
                        .to_string()
                })?;
            vec![prompt.text]
        }
    };
    let candidates = state.models.gate_candidates();
    if candidates.is_empty() {
        return Err(
            "Nothing to compare on yet: /login, link a plan, or start a local model".into(),
        );
    }
    let strongest = default_against(&candidates).map(|c| c.id.clone());
    let a_query = request.a.clone().unwrap_or_else(|| "gate".to_string());
    let b_query = match request.against.clone() {
        Some(query) => query,
        None if a_query.trim().to_ascii_lowercase().starts_with("gate") => {
            strongest.clone().ok_or("No model to compare Gate with")?
        }
        None => "gate".to_string(),
    };
    let (arm_a, arm_b) = resolve_arms(
        (&a_query, &b_query),
        &candidates,
        state.hub.gate.settings().delegate,
    )?;
    let judge = strongest.unwrap_or_else(|| match (&arm_a, &arm_b) {
        (ArmSpec::Model { model }, _) | (_, ArmSpec::Model { model }) => model.clone(),
        _ => String::new(),
    });
    let check = if examples.is_empty() {
        request.check.clone().or_else(|| {
            crate::harness::post_edit_check_from_env()
                .resolve(&workspace)
                .map(|check| check.command)
        })
    } else {
        None
    };
    let no_exploring = GateConfig {
        explore_rate: 0.0,
        ..*state.hub.gate.settings()
    };
    let expected = aura_agent_gate::pick(
        &state.hub.gate.stats(),
        &aura_agent_gate::Work::new(RouteLevel::Turn, category, None),
        &candidates,
        &no_exploring,
        &mut || 1.0,
    )
    .map(|decision| decision.model.id);
    let model_of = |spec: &ArmSpec| match spec {
        ArmSpec::Gate { .. } => expected.clone().unwrap_or_else(|| judge.clone()),
        ArmSpec::Model { model } => model.clone(),
    };
    let reads = |spec: &ArmSpec| match (spec, expected.as_deref()) {
        (ArmSpec::Gate { .. }, Some(model)) => {
            format!("{} (likely {})", arm_label(spec), route_label(model))
        }
        _ => arm_label(spec),
    };
    let (source, start, mut notes) = match replay {
        Some(replay) => (replay.source, replay.start, replay.notes),
        None if !examples.is_empty() => (
            BenchSource::Examples {
                tasks: examples.iter().map(|task| task.info()).collect(),
            },
            Vec::new(),
            Vec::new(),
        ),
        None => (BenchSource::Prompt, Vec::new(), Vec::new()),
    };
    if request.check.is_some() && !examples.is_empty() {
        notes.push("Example tasks use their own check commands; --check is ignored".to_string());
    }
    if request.learn {
        notes.push(format!(
            "Learn mode: Gate arms explore as routed turns do ({:.0}% of turns), so the runs \
             teach Gate about models it would not pick yet",
            state.hub.gate.settings().explore_rate * 100.0
        ));
    }
    let task = match prompts.first() {
        Some(first) => first.clone(),
        None => format!(
            "{} example coding task{}",
            examples.len(),
            if examples.len() == 1 { "" } else { "s" }
        ),
    };
    let plan = ComparePlan {
        task,
        prompts,
        examples,
        plan: request.plan,
        arms: [arm_a, arm_b],
        source,
        start: Arc::new(start),
        notes,
        judge: judge.clone(),
        repeats: request.repeats,
        check,
        budget_usd: request.budget_usd,
        candidates,
        classifier: state.models.classifier_model(),
        workspace,
        user: agent_id,
        root: state.compare.root.clone(),
        concurrency: state.compare.concurrency,
        learn: request.learn,
    };
    let session = match &plan.source {
        BenchSource::Session { title, .. } => Some(title.as_str()),
        BenchSource::Prompt | BenchSource::Examples { .. } => None,
    };
    let prompts_per_run = if plan.examples.is_empty() {
        plan.prompts.len()
    } else {
        let prompts: usize = plan.examples.iter().map(|t| t.prompts.len()).sum();
        prompts.div_ceil(plan.examples.len())
    };
    let text = EstimateView {
        task: &plan.task,
        prompts: plan.prompts.len(),
        session,
        examples: plan.source.examples(),
        a: reads(&plan.arms[0]),
        b: reads(&plan.arms[1]),
        judge: &plan.judge,
        repeats: plan.repeats,
        check: plan.check.as_deref(),
        budget_usd: plan.budget_usd,
        concurrency: plan.concurrency,
        estimate: estimate(
            &model_of(&plan.arms[0]),
            &model_of(&plan.arms[1]),
            &plan.judge,
            plan.runs_per_arm(),
            prompts_per_run,
        ),
        notes: &plan.notes,
    }
    .render();
    state.compare.pending = Some(plan);
    Ok(text)
}

/// `/compare go`: start the comparison waiting for it.
pub(super) async fn start(state: &mut LoopState<'_>) -> Result<String, String> {
    if let Some(running) = state.compare.running() {
        return Err(format!("{} is still running", running.name));
    }
    let plan = state
        .compare
        .pending
        .take()
        .ok_or("Nothing to start: /compare [prompt] prepares a comparison first")?;
    let host = {
        let mut registry = lock(&state.hub.registry);
        let parent = registry
            .find(plan.user)
            .map(|agent| agent.zns_id.clone())
            .ok_or("The agent that asked for the comparison is gone")?;
        registry
            .create_compare(&parent)
            .map_err(|e| format!("Could not register the comparison: {e:#}"))?
    };
    state.hub.send_agent_list().await;
    info!(
        compare = %host.name,
        repeats = plan.repeats,
        prompts = plan.prompts.len(),
        examples = plan.examples.len(),
        a = %plan.arms[0].label(),
        b = %plan.arms[1].label(),
        "Starting a comparison"
    );
    let runs = plan.runs_per_arm();
    let text = format!(
        "{} started: {} run{} per arm, up to {} at a time; they show in the Agents panel. \
         /compare cancel stops it.",
        host.name,
        runs,
        if runs == 1 { "" } else { "s" },
        plan.concurrency,
    );
    let cancel = CancellationToken::new();
    let task = tokio::spawn(compare_run::run(
        Arc::clone(&state.hub),
        state.commands.clone(),
        plan,
        host.clone(),
        cancel.clone(),
    ));
    state.compare.running = Some(Running {
        host: host.agent_id(),
        name: host.name,
        cancel,
        task,
    });
    Ok(text)
}

pub(super) fn cancel(state: &mut LoopState<'_>) -> String {
    if let Some(running) = state.compare.running() {
        running.cancel.cancel();
        return format!(
            "Stopping {}; runs under way end where they are",
            running.name
        );
    }
    if state.compare.pending.take().is_some() {
        return "Dropped the comparison".to_string();
    }
    "No comparison to cancel".to_string()
}

/// The arm `name` means in `payload`: `a` or `b`, what the arm runs on
/// (`gate`, a model or part of its name) when only one arm does, or
/// `other` for arm B as older comparisons called it.
fn arm_named(payload: &ComparePayload, name: &str) -> Option<CompareArm> {
    let name = name.trim().to_ascii_lowercase();
    match name.as_str() {
        "a" => return Some(CompareArm::A),
        "b" | "other" => return Some(CompareArm::B),
        _ => {}
    }
    let runs_on = |arm: &CompareArm| {
        let spec = payload.spec(*arm);
        let label = spec.label().to_ascii_lowercase();
        label == name
            || (spec.is_gate() && name == "gate")
            || model_key(&label) == name
            || (!spec.is_gate() && label.contains(&name))
    };
    let mut hits = CompareArm::BOTH.iter().copied().filter(runs_on);
    match (hits.next(), hits.next()) {
        (Some(arm), None) => Some(arm),
        _ => None,
    }
}

/// `/compare pick <arm>`: every run of the arm is rated good, every run of the
/// other's.
pub(super) async fn pick(
    state: &mut LoopState<'_>,
    host: Option<AgentId>,
    name: &str,
) -> Result<String, String> {
    let (host, compare, mut payload) = ended(state, host)?;
    let winner = arm_named(&payload, name).ok_or_else(|| {
        format!(
            "Pick a or b (A ran {}, B ran {})",
            payload.arm_a.label(),
            payload.arm_b.label()
        )
    })?;
    let mut rated = [0_usize; 2];
    for run in payload.runs.clone() {
        if run.agent_id == host {
            continue;
        }
        let kernel = kernel_of(state, run.agent_id)?;
        let Some(last) = super::gate_record::last_turn(kernel.store().as_ref(), run.agent_id)
        else {
            continue;
        };
        let signal = if run.arm == winner {
            OutcomeSignal::Good
        } else {
            OutcomeSignal::Bad
        };
        if super::gate_record::record(&kernel, run.agent_id, &last.outcome.with_signal(signal))
            .await
        {
            rated[run.arm.index()] += 1;
        }
    }
    payload.status = CompareStatus::Picked(winner);
    let kernel = kernel_of(state, host)?;
    save(
        &state.hub,
        &kernel,
        state.commands,
        (host, &compare),
        &payload,
    )
    .await;
    let (good, bad) = (rated[winner.index()], rated[winner.opposite().index()]);
    let letter = |arm: CompareArm| arm.as_str().to_uppercase();
    Ok(format!(
        "{compare}: picked {} ({}). Rated {good} {} run{} good and {bad} {} run{} bad; \
         Gate's table counts them.",
        letter(winner),
        payload.spec(winner).label(),
        letter(winner),
        if good == 1 { "" } else { "s" },
        letter(winner.opposite()),
        if bad == 1 { "" } else { "s" },
    ))
}

/// `/compare apply <run>`: put the run's changes in the user's
/// workspace, then remove the copies.
pub(super) async fn apply(
    state: &mut LoopState<'_>,
    host: Option<AgentId>,
    name: &str,
) -> Result<String, String> {
    let (host, compare, mut payload) = ended(state, host)?;
    if !payload.source.examples().is_empty() {
        return Err(format!(
            "{compare} ran example tasks in their own files; there is nothing to apply to your \
             workspace"
        ));
    }
    if !payload.status.has_copies() {
        return Err(format!("{compare}'s copies were already removed"));
    }
    let run = payload
        .run(name)
        .cloned()
        .ok_or_else(|| format!("No run {name} in {compare}; runs are named like a-1"))?;
    let shared = payload.clone();
    let apply_root = payload.apply_root.clone();
    let applied = tokio::task::spawn_blocking(move || {
        let patch = patches(&shared)
            .remove(&run.name)
            .unwrap_or_else(|| Err(format!("{} has no copy", run.name)))?;
        let files = crate::isolation::changes::files_in(&patch);
        crate::isolation::changes::apply_patch(&apply_root, &patch).map_err(|e| {
            format!(
                "{} did not apply to {}: {e}",
                run.name,
                apply_root.display()
            )
        })?;
        Ok::<_, String>((files, remove_copies(&shared)))
    })
    .await
    .map_err(|e| e.to_string())??;
    let (files, problems) = applied;
    payload.status = CompareStatus::Applied(name.to_string());
    let kernel = kernel_of(state, host)?;
    save(
        &state.hub,
        &kernel,
        state.commands,
        (host, &compare),
        &payload,
    )
    .await;
    Ok(format!(
        "{compare}: applied {name} ({files} file{}) to {}{}",
        if files == 1 { "" } else { "s" },
        payload.apply_root.display(),
        removed_line(&problems),
    ))
}

fn removed_line(problems: &[String]) -> String {
    if problems.is_empty() {
        "; removed the copies".to_string()
    } else {
        format!("; some copies were not removed: {}", problems.join("; "))
    }
}

/// `/compare dismiss`: remove the copies without applying anything.
pub(super) async fn dismiss(
    state: &mut LoopState<'_>,
    host: Option<AgentId>,
) -> Result<String, String> {
    if host.is_none() && state.compare.pending.take().is_some() {
        return Ok("Dropped the comparison".to_string());
    }
    let (host, compare, mut payload) = ended(state, host)?;
    if !payload.status.has_copies() {
        return Err(format!("{compare}'s copies were already removed"));
    }
    let shared = payload.clone();
    let problems = tokio::task::spawn_blocking(move || remove_copies(&shared))
        .await
        .map_err(|e| e.to_string())?;
    payload.status = CompareStatus::Dismissed;
    let kernel = kernel_of(state, host)?;
    save(
        &state.hub,
        &kernel,
        state.commands,
        (host, &compare),
        &payload,
    )
    .await;
    Ok(format!("{compare}: dismissed{}", removed_line(&problems)))
}

/// `/compare diff [run]`: the run's changes, or the first run of each
/// arm one after the other.
pub(super) async fn diff(
    state: &LoopState<'_>,
    ui: &AgentUi,
    host: Option<AgentId>,
    run: Option<&str>,
) -> Result<String, String> {
    let (_, compare, payload) = ended(state, host)?;
    if !payload.status.has_copies() {
        return Err(format!(
            "{compare}'s copies were removed, and their diffs with them"
        ));
    }
    let names: Vec<String> = match run {
        Some(name) => {
            payload
                .run(name)
                .ok_or_else(|| format!("No run {name} in {compare}"))?;
            vec![name.to_string()]
        }
        None => CompareArm::BOTH
            .iter()
            .filter_map(|arm| payload.arm_runs(*arm).next().map(|r| r.name.clone()))
            .collect(),
    };
    let shared = payload.clone();
    let mut all = tokio::task::spawn_blocking(move || patches(&shared))
        .await
        .map_err(|e| e.to_string())?;
    for name in names {
        let model = payload
            .run(&name)
            .map(|r| r.model.clone())
            .unwrap_or_default();
        let title = format!("{compare} {name} ({model})");
        match all.remove(&name) {
            Some(Ok(patch)) if patch.trim().is_empty() => {
                ui.send(system_message(format!("{title}: no changes")))
                    .await;
            }
            Some(Ok(patch)) => {
                ui.send(UiCommand::ShowDiff {
                    title,
                    files: split_patch(&patch),
                })
                .await;
            }
            Some(Err(problem)) => {
                ui.send(UiCommand::ShowWarning(format!("{title}: {problem}")))
                    .await;
            }
            None => {
                ui.send(UiCommand::ShowWarning(format!("{title}: it has no copy")))
                    .await;
            }
        }
    }
    Ok(String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arms_are_named_by_letter_or_by_what_they_run_on() {
        let mut payload = ComparePayload {
            system_kind: aura_core_types::SystemKind::Compare,
            task: String::new(),
            plan: None,
            arm_a: ArmSpec::Gate { delegate: true },
            arm_b: ArmSpec::Model {
                model: "claude/claude-opus-4-7".to_string(),
            },
            against: "claude/claude-opus-4-7".to_string(),
            prompts: vec![String::new()],
            source: BenchSource::Prompt,
            repeats: 1,
            check: None,
            budget_usd: None,
            requested_by: AgentId::generate(),
            apply_root: PathBuf::new(),
            started_ms: 0,
            runs: Vec::new(),
            judgments: Vec::new(),
            notes: Vec::new(),
            status: CompareStatus::Done,
        };
        assert_eq!(arm_named(&payload, "a"), Some(CompareArm::A));
        assert_eq!(arm_named(&payload, "B"), Some(CompareArm::B));
        assert_eq!(arm_named(&payload, "gate"), Some(CompareArm::A));
        assert_eq!(arm_named(&payload, "other"), Some(CompareArm::B));
        assert_eq!(arm_named(&payload, "opus"), Some(CompareArm::B));
        assert_eq!(arm_named(&payload, "claude-opus-4-7"), Some(CompareArm::B));
        assert_eq!(arm_named(&payload, "sonnet"), None);
        payload.arm_a = ArmSpec::Gate { delegate: false };
        assert_eq!(arm_named(&payload, "gate"), Some(CompareArm::A));

        payload.arm_a = ArmSpec::Model {
            model: "claude/claude-sonnet-4-6".to_string(),
        };
        assert_eq!(arm_named(&payload, "sonnet"), Some(CompareArm::A));
        assert_eq!(arm_named(&payload, "claude"), None);
        assert_eq!(arm_named(&payload, "gate"), None);
    }
}
