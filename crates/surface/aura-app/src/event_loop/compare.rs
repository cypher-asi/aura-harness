//! `/compare`: run the user's own task (or plan) with Gate and with
//! another model side by side, then pick or apply the better run.
//!
//! `/compare [prompt]` prepares a comparison and shows what it should
//! cost; `/compare go` starts it (see [`super::compare_run`]). Each
//! comparison is an agent (`Compare #n`) under the agent that asked,
//! with its runs under it; its state is the latest `compare` system
//! record on its own record, which is also where `/compare` lists past
//! comparisons from. Copies are removed by `/compare apply` and
//! `/compare dismiss`.

use super::agent_events::AgentUi;
use super::compare_run::{self, patches, remove_copies, write_record, ComparePlan};
use super::compare_view::{self, split_patch, EstimateView};
use super::handlers::system_message;
use super::subagents::lock;
use super::LoopState;
use aura_agent_gate::compare::{
    default_against, estimate, parse, resolve_opponent, CompareCommand, CompareRequest, Opponent,
    DEFAULT_CONCURRENCY,
};
use aura_agent_kernel::Kernel;
use aura_config::GateConfig;
use aura_context_prompts::model_messages::build_plan::build_plan_prompt;
use aura_core_types::{
    AgentId, CompareArm, ComparePayload, CompareStatus, OutcomeSignal, PromptCategory, RouteLevel,
};
use aura_model::model_catalog::model_key;
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
        CompareCommand::Start(request) => prepare(state, agent_id, request).await,
        CompareCommand::Go => start(state).await,
        CompareCommand::Cancel => Ok(cancel(state)),
        CompareCommand::Pick(arm) => pick(state, &arm).await,
        CompareCommand::Apply(run) => apply(state, &run).await,
        CompareCommand::Diff(run) => diff(state, &ui, run.as_deref()).await,
        CompareCommand::Dismiss => dismiss(state).await,
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
fn comparisons(state: &LoopState<'_>) -> Vec<(AgentId, String, Option<ComparePayload>)> {
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

/// The latest comparison that has ended, for pick, apply, diff and
/// dismiss.
fn latest(state: &LoopState<'_>) -> Result<(AgentId, String, ComparePayload), String> {
    let (host, name, payload) = comparisons(state)
        .into_iter()
        .rev()
        .find_map(|(id, name, payload)| Some((id, name, payload?)))
        .ok_or_else(|| "No comparison yet; /compare [prompt] runs one".to_string())?;
    if state.compare.running().is_some_and(|r| r.host == host)
        || payload.status == CompareStatus::Running
    {
        return Err(format!("{name} is still running; wait for it to end"));
    }
    Ok((host, name, payload))
}

/// The kernel of `agent_id`, opening its session when nothing has.
fn kernel_of(state: &mut LoopState<'_>, agent_id: AgentId) -> Result<Arc<Kernel>, String> {
    if let Some(kernel) = state.kernel_for(agent_id) {
        return Ok(kernel);
    }
    let session = super::agents::open_session(state, agent_id)?;
    let kernel = Arc::clone(&session.kernel);
    state.idle.insert(agent_id, session);
    Ok(kernel)
}

/// Prepare the comparison `request` asks for from `agent_id`'s chat and
/// show its estimate.
async fn prepare(
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
    let task = match (&request.plan, &request.prompt) {
        (Some(plan), _) => {
            if !workspace.join(plan).is_file() {
                return Err(format!("No plan at {plan} in {}", workspace.display()));
            }
            build_plan_prompt(plan)
        }
        (None, Some(prompt)) => prompt.clone(),
        (None, None) => {
            let prompt = last
                .and_then(|turn| {
                    category = turn.outcome.category;
                    turn.prompt
                })
                .ok_or_else(|| {
                    "No prompt to compare yet: give one, e.g. /compare fix the failing test"
                        .to_string()
                })?;
            prompt.text
        }
    };
    let candidates = state.models.gate_candidates();
    if candidates.is_empty() {
        return Err(
            "Nothing to compare on yet: /login, link a plan, or start a local model".into(),
        );
    }
    let against = match request.against.as_deref() {
        Some(query) => resolve_opponent(query, &candidates)?,
        None => Opponent::Model(
            default_against(&candidates)
                .ok_or("No model to compare Gate with")?
                .id
                .clone(),
        ),
    };
    let judge =
        default_against(&candidates).map_or_else(|| against.label().to_string(), |c| c.id.clone());
    // Against Gate without enforced delegation, Gate's arm always enforces
    // it, whatever AURA_GATE_DELEGATE says, so the comparison measures
    // delegation itself.
    let delegate = against == Opponent::GateNoDelegate || state.hub.gate.settings().delegate;
    let check = request.check.clone().or_else(|| {
        crate::harness::post_edit_check_from_env()
            .resolve(&workspace)
            .map(|check| check.command)
    });
    let no_exploring = GateConfig {
        explore_rate: 0.0,
        ..*state.hub.gate.settings()
    };
    let expected = aura_agent_gate::pick(
        &state.hub.gate.stats(),
        RouteLevel::Turn,
        category,
        &candidates,
        &no_exploring,
        &mut || 1.0,
    )
    .map(|decision| decision.model.id);
    let other_model = match &against {
        Opponent::Model(model) => model.clone(),
        Opponent::GateNoDelegate => expected.clone().unwrap_or_else(|| judge.clone()),
    };
    let plan = ComparePlan {
        task,
        plan: request.plan,
        against,
        delegate,
        judge,
        repeats: request.repeats,
        check,
        budget_usd: request.budget_usd,
        candidates,
        classifier: state.models.classifier_model(),
        workspace,
        user: agent_id,
        root: state.compare.root.clone(),
        concurrency: state.compare.concurrency,
    };
    let text = EstimateView {
        task: &plan.task,
        gate_model: expected.as_deref(),
        against: plan.against.label(),
        judge: &plan.judge,
        repeats: plan.repeats,
        check: plan.check.as_deref(),
        budget_usd: plan.budget_usd,
        concurrency: plan.concurrency,
        estimate: estimate(
            expected.as_deref().unwrap_or(&other_model),
            &other_model,
            &plan.judge,
            plan.repeats,
        ),
    }
    .render();
    state.compare.pending = Some(plan);
    Ok(text)
}

/// `/compare go`: start the comparison waiting for it.
async fn start(state: &mut LoopState<'_>) -> Result<String, String> {
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
    info!(compare = %host.name, repeats = plan.repeats, against = %plan.against.label(), "Starting a comparison");
    let text = format!(
        "{} started: {} run{} per arm, up to {} at a time; they show in the Agents panel. \
         /compare cancel stops it.",
        host.name,
        plan.repeats,
        if plan.repeats == 1 { "" } else { "s" },
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

fn cancel(state: &mut LoopState<'_>) -> String {
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

/// The arm `name` means in `payload`: `gate`, or `other` or the other
/// arm's model.
fn arm_named(payload: &ComparePayload, name: &str) -> Option<CompareArm> {
    let against = payload.against.to_ascii_lowercase();
    match name {
        "gate" => Some(CompareArm::Gate),
        "other" => Some(CompareArm::Other),
        _ if against == name || model_key(&against) == name || against.contains(name) => {
            Some(CompareArm::Other)
        }
        _ => None,
    }
}

/// `/compare pick <arm>`: every run of the arm is rated good, every run of the
/// other's.
async fn pick(state: &mut LoopState<'_>, name: &str) -> Result<String, String> {
    let (host, compare, mut payload) = latest(state)?;
    let winner = arm_named(&payload, name)
        .ok_or_else(|| format!("Pick gate or other (other ran {})", payload.against))?;
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
            rated[usize::from(run.arm == CompareArm::Other)] += 1;
        }
    }
    payload.status = CompareStatus::Picked(winner);
    let kernel = kernel_of(state, host)?;
    write_record(&kernel, host, &payload).await;
    let (good, bad) = match winner {
        CompareArm::Gate => (rated[0], rated[1]),
        CompareArm::Other => (rated[1], rated[0]),
    };
    Ok(format!(
        "{compare}: picked {winner}. Rated {good} {winner} run{} good and {bad} {} run{} bad; \
         Gate's table counts them.",
        if good == 1 { "" } else { "s" },
        winner.opposite(),
        if bad == 1 { "" } else { "s" },
    ))
}

/// `/compare apply <run>`: put the run's changes in the user's
/// workspace, then remove the copies.
async fn apply(state: &mut LoopState<'_>, name: &str) -> Result<String, String> {
    let (host, compare, mut payload) = latest(state)?;
    if !payload.status.has_copies() {
        return Err(format!("{compare}'s copies were already removed"));
    }
    let run = payload
        .run(name)
        .cloned()
        .ok_or_else(|| format!("No run {name} in {compare}; runs are named like gate-1"))?;
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
    write_record(&kernel, host, &payload).await;
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
async fn dismiss(state: &mut LoopState<'_>) -> Result<String, String> {
    if state.compare.pending.take().is_some() {
        return Ok("Dropped the comparison".to_string());
    }
    let (host, compare, mut payload) = latest(state)?;
    if !payload.status.has_copies() {
        return Err(format!("{compare}'s copies were already removed"));
    }
    let shared = payload.clone();
    let problems = tokio::task::spawn_blocking(move || remove_copies(&shared))
        .await
        .map_err(|e| e.to_string())?;
    payload.status = CompareStatus::Dismissed;
    let kernel = kernel_of(state, host)?;
    write_record(&kernel, host, &payload).await;
    Ok(format!("{compare}: dismissed{}", removed_line(&problems)))
}

/// `/compare diff [run]`: the run's changes, or the first run of each
/// arm one after the other.
async fn diff(state: &LoopState<'_>, ui: &AgentUi, run: Option<&str>) -> Result<String, String> {
    let (_, compare, payload) = latest(state)?;
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
    fn arms_are_named_by_side_or_by_the_other_model() {
        let payload = ComparePayload {
            system_kind: aura_core_types::SystemKind::Compare,
            task: String::new(),
            plan: None,
            against: "claude/claude-opus-4-7".to_string(),
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
        assert_eq!(arm_named(&payload, "gate"), Some(CompareArm::Gate));
        assert_eq!(arm_named(&payload, "other"), Some(CompareArm::Other));
        assert_eq!(arm_named(&payload, "opus"), Some(CompareArm::Other));
        assert_eq!(
            arm_named(&payload, "claude-opus-4-7"),
            Some(CompareArm::Other)
        );
        assert_eq!(arm_named(&payload, "sonnet"), None);
    }
}
