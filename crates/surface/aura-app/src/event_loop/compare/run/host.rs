//! The comparison on its own agent: its session, the agents of its arms
//! and runs, its record from the first state to the last, and how it
//! ended, shown in the user's chat.

use super::arm::{arm_name, arm_routing};
use super::context::{Context, Ran};
use super::judge::judge_all;
use super::labels::Labels;
use super::plan::ComparePlan;
use super::schedule::run_all;
use super::setup::set_up;
use super::start::compare_cache_salt;
use crate::agent_registry::RegisteredAgent;
use crate::event_loop::agent_events::AgentUi;
use crate::event_loop::agents::AgentSession;
use crate::event_loop::bench;
use crate::event_loop::compare::view::render;
use crate::event_loop::handlers::system_message;
use crate::event_loop::processes::now_ms;
use crate::event_loop::subagents::{lock, AgentHub};
use crate::isolation::changes;
use aura_agent_kernel::Kernel;
use aura_core_types::{
    AgentId, CompareArm, ComparePayload, CompareStatus, SystemKind, Transaction,
};
use aura_gate::compare::{plan_shares, run_name, PlanReading};
use aura_gate::live::{self, GateLive, RunOf};
use aura_model::ModelCallPurpose;
use aura_ui_terminal::events::BenchTaskSetup;
use aura_ui_terminal::UiCommand;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Run the comparison `plan` on the agent `host`, until it ends or
/// `cancel` stops it, and show the result in the user's chat.
pub(crate) async fn run(
    hub: Arc<AgentHub>,
    commands: mpsc::Sender<UiCommand>,
    plan: ComparePlan,
    host: RegisteredAgent,
    cancel: CancellationToken,
) {
    let user_ui = AgentUi::new(commands.clone(), plan.user);
    let Some(session) = open(&hub, &host, &user_ui).await else {
        return;
    };
    let repo = repo_of(&plan.workspace).await;
    let dir = plan
        .root
        .join(host.zns_id.rsplit('/').next().unwrap_or("compare"));
    let mut payload = started(&plan, repo.as_deref());
    if let Some(note) = copies_inside(&plan, &dir, &payload.apply_root) {
        payload.notes.push(note);
        payload.status = CompareStatus::Cancelled;
        finish(&hub, &commands, &user_ui, &host, session, &payload).await;
        return;
    }
    let ids = (host.agent_id(), host.name.as_str());
    save(&hub, &session.kernel, &commands, ids, &payload).await;
    let arms = register_arms(&hub, &plan, &host);
    let agents = register_runs(&hub, &plan, &arms);
    hub.send_agent_list().await;
    let (states, setup) = watch::channel(vec![BenchTaskSetup::Queued; plan.examples.len()]);
    tokio::spawn(set_up(
        plan.examples.clone(),
        dir.clone(),
        states,
        cancel.clone(),
    ));
    let routing = CompareArm::BOTH.map(|arm| arm_routing(&plan.arms[arm.index()], &plan));
    let gates = arm_gates(&hub.gate, host.agent_id());
    let ctx = Arc::new(Context {
        hub,
        commands,
        plan,
        host,
        kernel: Arc::clone(&session.kernel),
        arms,
        routing,
        gates,
        dir,
        repo,
        cancel,
        setup,
        agents,
        live: Mutex::default(),
        labels: Labels::default(),
    });
    let ran = run_all(&ctx, &session, &user_ui, &mut payload).await;
    conclude(&ctx, session, &user_ui, ran, payload).await;
}

/// Each arm's own view of Gate in the comparison on agent `compare`, A
/// first: A's a snapshot of `shared`, B's one of A's, so both start from
/// the same table, each drawing from a generator seeded from its arm's
/// cache salt.
fn arm_gates(shared: &GateLive, compare: AgentId) -> [Arc<GateLive>; 2] {
    let a = shared.snapshot(arm_seed(compare, CompareArm::A));
    let b = a.snapshot(arm_seed(compare, CompareArm::B));
    [Arc::new(a), Arc::new(b)]
}

/// The seed of `arm`'s view of Gate in the comparison on agent
/// `compare`: its cache salt read as hex, else the first eight bytes of a
/// BLAKE3 hash of the salt.
fn arm_seed(compare: AgentId, arm: CompareArm) -> u64 {
    let salt = compare_cache_salt(compare, arm);
    u64::from_str_radix(&salt, 16).unwrap_or_else(|_| {
        let hash = blake3::hash(salt.as_bytes());
        hash.as_bytes()
            .first_chunk::<8>()
            .map_or(0, |bytes| u64::from_le_bytes(*bytes))
    })
}

/// The session of the comparison's agent `host`; `None`, with why shown
/// to the user, when it can't be opened.
async fn open(hub: &AgentHub, host: &RegisteredAgent, user_ui: &AgentUi) -> Option<AgentSession> {
    let folder = lock(&hub.registry).project_folder(host.agent_id());
    match hub.factory.open(host, Vec::new(), folder) {
        Ok(session) => Some(session),
        Err(e) => {
            let problem = format!("Could not start {}: {e:#}", host.name);
            user_ui.send(UiCommand::ShowError(problem)).await;
            None
        }
    }
}

/// Root of the git repository `workspace` is in, if it is in one.
async fn repo_of(workspace: &Path) -> Option<PathBuf> {
    let workspace = workspace.to_path_buf();
    tokio::task::spawn_blocking(move || changes::repo_root(&workspace))
        .await
        .ok()
        .flatten()
}

/// Why `plan`'s copies can't go in `dir`: they would land inside
/// `apply_root`, the folder they copy. `None` when they can, or when the
/// work is example tasks, whose copies are made from their own files.
fn copies_inside(plan: &ComparePlan, dir: &Path, apply_root: &Path) -> Option<String> {
    (plan.examples.is_empty() && dir.starts_with(apply_root)).then(|| {
        format!(
            "The copies would land inside {}, the folder they copy (AURA_HOME is in it); \
             set AURA_HOME elsewhere to compare here",
            apply_root.display()
        )
    })
}

/// The record of `plan` as it starts, its runs applied to `repo` when
/// the workspace is in one.
fn started(plan: &ComparePlan, repo: Option<&Path>) -> ComparePayload {
    ComparePayload {
        system_kind: SystemKind::Compare,
        task: plan.task.clone(),
        plan: plan.plan.clone(),
        arm_a: plan.arms[0].clone(),
        arm_b: plan.arms[1].clone(),
        against: plan.arms[1].label().to_string(),
        prompts: plan.prompts.clone(),
        source: plan.source.clone(),
        repeats: plan.repeats,
        check: plan.check.clone(),
        budget_usd: plan.budget_usd,
        classifier: plan.classifier.clone(),
        classify_tokens: Vec::new(),
        requested_by: plan.user,
        apply_root: repo.map_or_else(|| plan.workspace.clone(), Path::to_path_buf),
        started_ms: now_ms(),
        runs: Vec::new(),
        judgments: Vec::new(),
        notes: plan.notes.clone(),
        status: CompareStatus::Running,
    }
}

/// Register under `host` the agents grouping each arm's runs; returns
/// their ZNS ids, A first. An arm that can't be registered has its runs
/// go under the comparison.
fn register_arms(hub: &AgentHub, plan: &ComparePlan, host: &RegisteredAgent) -> [String; 2] {
    CompareArm::BOTH.map(|arm| {
        let name = arm_name(arm, &plan.arms[arm.index()]);
        match lock(&hub.registry).create_compare_arm(&name, &host.zns_id) {
            Ok(agent) => agent.zns_id,
            Err(e) => {
                warn!(arm = %arm, error = %e, "Could not register an arm; its runs go under the comparison");
                host.zns_id.clone()
            }
        }
    })
}

/// Register each of `plan`'s runs under its arm's agent in `arms`, so the
/// Agents panel lists them waiting; returns them by run name.
fn register_runs(
    hub: &AgentHub,
    plan: &ComparePlan,
    arms: &[String; 2],
) -> HashMap<String, RegisteredAgent> {
    let mut agents = HashMap::new();
    for n in 1..=plan.runs_per_arm() {
        for arm in CompareArm::BOTH {
            let name = run_name(arm, n);
            match lock(&hub.registry).create_child(&name, &arms[arm.index()], None) {
                Ok(agent) => {
                    agents.insert(name, agent);
                }
                Err(e) => warn!(run = %name, error = %e, "Could not register a run up front"),
            }
        }
    }
    agents
}

/// Settle the comparison once its runs in `ran` have ended: record what
/// their checks proved, note what kept runs from running, share out the
/// plan usage, count the classifier's calls, judge the runs unless the
/// user stopped the comparison, and finish it.
async fn conclude(
    ctx: &Context,
    session: AgentSession,
    user_ui: &AgentUi,
    mut ran: Vec<Ran>,
    mut payload: ComparePayload,
) {
    record_checks(ctx, &mut ran).await;
    // What the checks proved may change which signs have work reviewed.
    ctx.hub.gate.learn_tiers();
    forget_unused(ctx, &ran);
    for (task, state) in ctx.plan.examples.iter().zip(ctx.setup.borrow().iter()) {
        if let BenchTaskSetup::Failed(why) = state {
            payload
                .notes
                .push(format!("writing the files of {}: {why}", task.id));
        }
    }
    ran.sort_by_key(|r| r.order);
    let readings: Vec<&[PlanReading]> = ran.iter().map(|r| r.readings.as_slice()).collect();
    let shares = plan_shares(&readings);
    for (r, share) in ran.iter_mut().zip(shares) {
        r.run.plan_usage = share;
    }
    payload
        .notes
        .extend(ran.iter_mut().filter_map(|r| r.note.take()));
    payload.classify_tokens =
        (ctx.hub).own_tokens_for(ctx.host.agent_id(), ModelCallPurpose::Classify);
    if ctx.cancel.is_cancelled() {
        payload.status = CompareStatus::Cancelled;
    } else {
        let judging = format!("{}: judging the runs", ctx.host.name);
        user_ui.send(system_message(judging)).await;
        payload.judgments = judge_all(ctx, &session, &ran).await;
        payload.status = CompareStatus::Done;
    }
    payload.runs = ran.into_iter().map(|r| r.run).collect();
    finish(
        &ctx.hub,
        &ctx.commands,
        user_ui,
        &ctx.host,
        session,
        &payload,
    )
    .await;
}

/// Record each run's check verdict on its kernel as a later `Outcome` of
/// its last turn, now that every run has ended, and give it to Gate as
/// what the turn's work truly was.
async fn record_checks(ctx: &Context, ran: &mut [Ran]) {
    for r in ran {
        let Some((kernel, ended)) = r.checked.take() else {
            continue;
        };
        if let Some(passed) = ended.success {
            let of = RunOf {
                compare: &ctx.host.name,
                run: &r.run.name,
                task: r.run.task.as_deref(),
                copy: Some(&r.run.copy),
            };
            ctx.hub
                .gate
                .log_truth(r.run.agent_id, ended.prompt_tx, passed, of);
        }
        live::record(&kernel, r.run.agent_id, &ended).await;
    }
}

/// Drop the agents registered up front for runs that never started.
fn forget_unused(ctx: &Context, ran: &[Ran]) {
    for agent in ctx.agents.values() {
        let id = agent.agent_id();
        if ran.iter().any(|r| r.run.agent_id == id) {
            continue;
        }
        if let Err(e) = lock(&ctx.hub.registry).forget(id) {
            warn!(run = %agent.name, error = %e, "Could not drop a run that did not start");
        }
    }
}

/// Record how the comparison ended, show it, and hand its agent back.
async fn finish(
    hub: &AgentHub,
    commands: &mpsc::Sender<UiCommand>,
    user_ui: &AgentUi,
    host: &RegisteredAgent,
    session: AgentSession,
    payload: &ComparePayload,
) {
    let ids = (host.agent_id(), host.name.as_str());
    save(hub, &session.kernel, commands, ids, payload).await;
    info!(compare = %host.name, runs = payload.runs.len(), "Comparison ended");
    user_ui
        .send(system_message(render(&host.name, payload)))
        .await;
    hub.park(session);
    hub.send_agent_list().await;
}

/// Write `payload` to the record of the comparison's agent `agent_id`
/// (named `name`) and show it in the BENCH tab. A failure to record is
/// logged.
pub(crate) async fn save(
    hub: &AgentHub,
    kernel: &Kernel,
    commands: &mpsc::Sender<UiCommand>,
    (agent_id, name): (AgentId, &str),
    payload: &ComparePayload,
) -> bool {
    let recorded = record(kernel, agent_id, payload).await;
    let root = lock(&hub.registry).root(agent_id);
    let view = bench::view(agent_id, root, name, payload);
    let _ = commands.send(UiCommand::BenchUpsert(Box::new(view))).await;
    recorded
}

/// Write `payload` to the record of the comparison's agent `agent_id`.
/// A failure is logged.
pub(super) async fn record(kernel: &Kernel, agent_id: AgentId, payload: &ComparePayload) -> bool {
    match kernel
        .process_direct(Transaction::compare(agent_id, payload))
        .await
    {
        Ok(_) => true,
        Err(e) => {
            warn!(%agent_id, error = %e, "Failed to record the comparison");
            false
        }
    }
}
