//! Running a `/compare`: every run of both arms in its own copy of the
//! workspace, then the blind judge, then the record and the view.
//!
//! Each run is an agent of its own, registered under the comparison's
//! agent with the copy as its folder, so it shows in the Agents panel
//! and the Processes tab while it works, its subtasks work in the same
//! copy, and its records feed Gate's table like any turn. Its turn runs
//! on the same turn driver as the terminal's turns and `task` spawns:
//!
//! - the Gate arm's turn goes where Gate sends it with exploration off,
//!   and its subtasks are routed the same way;
//! - the other arm's turn and subtasks run on the model it is pinned to.
//!
//! The prompt is recorded with route reason `compare` and the turn ends
//! with its `Outcome`; the check command then runs in the copy as a
//! `run_command` call through the run's kernel, so it is in the run's log,
//! and its verdict is recorded as a later `Outcome`, so it counts in the
//! table.
//! Cost and tokens per model come from the records of the run and its
//! subtasks: every model call there, classifier calls included.

use super::agent_events::AgentUi;
use super::agents::AgentSession;
use super::compare_view::{duration, money, render};
use super::gate_record;
use super::handlers::{handle_agent_success, run_agent_turn, system_message, turn_summary};
use super::processes::{now_ms, turn_id};
use super::steering::Steering;
use super::subagents::{lock, AgentHub, ArmRouting};
use super::task_check;
use super::turns::{ended_signal, record_prompt};
use crate::agent_registry::RegisteredAgent;
use aura_agent::AgentLoop;
use aura_agent_gate::compare::{list_price, run_name, tally, Opponent, Schedule};
use aura_agent_gate::{judge, Candidate};
use aura_agent_kernel::Kernel;
use aura_core_types::{
    AgentId, CompareArm, CompareCopy, CompareJudgment, ComparePayload, CompareRun, CompareStatus,
    ModelTokens, OutcomePayload, OutcomeSignal, PromptCategory, PromptPayload, Route, RouteLevel,
    RouteReason, SystemKind, Transaction,
};
use aura_exec_isolation::changes::{self, BUILD_OUTPUT};
use aura_exec_isolation::{
    CopyIsolation, IsolatedWorkspace, Isolation, IsolationStrategy, WorktreeIsolation,
};
use aura_model_reasoner::{Message, Usage};
use aura_store_db::ReadStore;
use aura_terminal::events::ProcessStatus;
use aura_terminal::{PermissionMode, UiCommand};
use aura_tools::plan_tools::{is_plan_path, todo_items, PlanFile};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// `ToolProposal::source` of a run's check command.
const CHECK_SOURCE: &str = "compare_check";

/// Longest a judge call may take.
const JUDGE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Judge reasoning kept in the record, in characters.
const MAX_REASONING: usize = 2_000;

/// Record entries read per scan when adding up a run's model calls.
const SCAN_BATCH: usize = 500;

/// A comparison as the user confirmed it.
#[derive(Debug, Clone)]
pub(super) struct ComparePlan {
    /// The prompt every run gets (the build prompt for a plan)
    pub task: String,
    /// Plan file each run builds, relative to the workspace
    pub plan: Option<String>,
    /// What the other arm runs on
    pub against: Opponent,
    /// Whether the Gate arm's lead must delegate
    pub delegate: bool,
    /// Model that judges
    pub judge: String,
    pub repeats: u32,
    pub check: Option<String>,
    pub budget_usd: Option<f64>,
    /// What Gate routes the Gate arm among
    pub candidates: Vec<Candidate>,
    /// Model that classifies the runs' prompts
    pub classifier: String,
    /// The user's folder the copies are made from
    pub workspace: PathBuf,
    /// The user's agent that asked
    pub user: AgentId,
    /// Where comparisons keep their copies (`AURA_HOME/compare`)
    pub root: PathBuf,
    /// Runs that may work at once
    pub concurrency: usize,
}

/// What the runs of one comparison share.
struct Context {
    hub: Arc<AgentHub>,
    commands: mpsc::Sender<UiCommand>,
    plan: ComparePlan,
    host: RegisteredAgent,
    /// Where this comparison's copies go
    dir: PathBuf,
    /// Root of the user's git repository, when the workspace is in one
    repo: Option<PathBuf>,
    cancel: CancellationToken,
}

/// A run that ended, with what the judge needs from it.
struct Ran {
    run: CompareRun,
    /// Its order: run number, then arm
    order: (u32, CompareArm),
    reply: String,
    patch: String,
    note: Option<String>,
}

/// Write `payload` to the record of the comparison's agent `agent_id`.
/// A failure is logged.
pub(super) async fn write_record(
    kernel: &Kernel,
    agent_id: AgentId,
    payload: &ComparePayload,
) -> bool {
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

/// Run the comparison `plan` on the agent `host`, until it ends or
/// `cancel` stops it, and show the result in the user's chat.
pub(super) async fn run(
    hub: Arc<AgentHub>,
    commands: mpsc::Sender<UiCommand>,
    plan: ComparePlan,
    host: RegisteredAgent,
    cancel: CancellationToken,
) {
    let user_ui = AgentUi::new(commands.clone(), plan.user);
    let host_id = host.agent_id();
    let folder = lock(&hub.registry).project_folder(host_id);
    let session = match hub.factory.open(&host, Vec::new(), folder) {
        Ok(session) => session,
        Err(e) => {
            user_ui
                .send(UiCommand::ShowError(format!(
                    "Could not start {}: {e:#}",
                    host.name
                )))
                .await;
            return;
        }
    };
    let workspace = plan.workspace.clone();
    let repo = tokio::task::spawn_blocking(move || changes::repo_root(&workspace))
        .await
        .ok()
        .flatten();
    let dir = plan
        .root
        .join(host.zns_id.rsplit('/').next().unwrap_or("compare"));
    let mut payload = ComparePayload {
        system_kind: SystemKind::Compare,
        task: plan.task.clone(),
        plan: plan.plan.clone(),
        against: plan.against.label().to_string(),
        repeats: plan.repeats,
        check: plan.check.clone(),
        budget_usd: plan.budget_usd,
        requested_by: plan.user,
        apply_root: repo.clone().unwrap_or_else(|| plan.workspace.clone()),
        started_ms: now_ms(),
        runs: Vec::new(),
        judgments: Vec::new(),
        notes: Vec::new(),
        status: CompareStatus::Running,
    };
    if dir.starts_with(&payload.apply_root) {
        payload.notes.push(format!(
            "The copies would land inside {}, the folder they copy (AURA_HOME is in it); \
             set AURA_HOME elsewhere to compare here",
            payload.apply_root.display()
        ));
        payload.status = CompareStatus::Cancelled;
        finish(&hub, &user_ui, &host, session, &payload).await;
        return;
    }
    write_record(&session.kernel, host_id, &payload).await;

    let ctx = Arc::new(Context {
        hub: Arc::clone(&hub),
        commands,
        plan,
        host: host.clone(),
        dir,
        repo,
        cancel,
    });
    let mut ran = run_all(&ctx, &session, &user_ui, &mut payload).await;
    ran.sort_by_key(|r| r.order);
    payload.runs = ran.iter().map(|r| r.run.clone()).collect();
    payload
        .notes
        .extend(ran.iter().filter_map(|r| r.note.clone()));

    if ctx.cancel.is_cancelled() {
        payload.status = CompareStatus::Cancelled;
    } else {
        user_ui
            .send(system_message(format!("{}: judging the runs", host.name)))
            .await;
        payload.judgments = judge_all(&ctx, &session, &ran).await;
        payload.status = CompareStatus::Done;
    }
    finish(&hub, &user_ui, &host, session, &payload).await;
}

/// Record how the comparison ended, show it, and hand its agent back.
async fn finish(
    hub: &AgentHub,
    user_ui: &AgentUi,
    host: &RegisteredAgent,
    session: AgentSession,
    payload: &ComparePayload,
) {
    write_record(&session.kernel, host.agent_id(), payload).await;
    info!(compare = %host.name, runs = payload.runs.len(), "Comparison ended");
    user_ui
        .send(system_message(render(&host.name, payload)))
        .await;
    hub.park(session);
    hub.send_agent_list().await;
}

/// Start the runs as the schedule allows and wait for them all,
/// recording each as it ends.
async fn run_all(
    ctx: &Arc<Context>,
    session: &AgentSession,
    user_ui: &AgentUi,
    payload: &mut ComparePayload,
) -> Vec<Ran> {
    let plan = &ctx.plan;
    let mut schedule = Schedule::new(plan.repeats, plan.concurrency, plan.budget_usd);
    let mut tasks = JoinSet::new();
    let mut ran = Vec::new();
    loop {
        if !ctx.cancel.is_cancelled() {
            while let Some((arm, n)) = schedule.start_next() {
                tasks.spawn(run_one(Arc::clone(ctx), arm, n));
            }
        }
        let Some(joined) = tasks.join_next().await else {
            break;
        };
        let Ok(done) = joined else {
            warn!("A compare run panicked");
            schedule.finished(0.0);
            continue;
        };
        schedule.finished(done.run.cost_usd);
        user_ui
            .send(system_message(progress(&ctx.host.name, &done.run)))
            .await;
        payload.runs.push(done.run.clone());
        write_record(&session.kernel, ctx.host.agent_id(), payload).await;
        ran.push(done);
    }
    let skipped = schedule.drop_pending().len();
    if skipped > 0 && !ctx.cancel.is_cancelled() {
        payload.notes.push(format!(
            "The budget of {} was spent after {}; {skipped} run{} did not start",
            money(plan.budget_usd.unwrap_or_default()),
            money(schedule.spent_usd()),
            if skipped == 1 { "" } else { "s" },
        ));
    }
    ran
}

/// The line the user's chat gets when `run` ends.
fn progress(compare: &str, run: &CompareRun) -> String {
    let check = match run.check_passed {
        Some(true) => ", check passed",
        Some(false) => ", check failed",
        None => "",
    };
    match &run.error {
        Some(error) => format!("{compare}: {} stopped: {error}", run.name),
        None => format!(
            "{compare}: {} done in {}, {}{check}",
            run.name,
            duration(run.wall_ms),
            money(run.cost_usd)
        ),
    }
}

/// A run that never got going, with why.
fn not_run(ctx: &Context, arm: CompareArm, n: u32, error: String) -> Ran {
    Ran {
        run: CompareRun {
            name: run_name(arm, n),
            arm,
            agent_id: ctx.host.agent_id(),
            prompt_tx: None,
            model: String::new(),
            copy: CompareCopy {
                root: PathBuf::new(),
                workdir: PathBuf::new(),
                git_dir: None,
                baseline: String::new(),
            },
            wall_ms: 0,
            cost_usd: 0.0,
            tokens: Vec::new(),
            check_passed: None,
            files_changed: 0,
            error: Some(error),
        },
        order: (n, arm),
        reply: String::new(),
        patch: String::new(),
        note: None,
    }
}

/// Make the copy run `name` works in: a detached worktree of the user's
/// repository with their uncommitted work carried over, else a copy of
/// their folder without build output. Returns it with a note when the
/// uncommitted work could not be carried.
fn prepare_copy(
    dir: &Path,
    workspace: &Path,
    repo: Option<&Path>,
    plan_file: Option<&str>,
    name: &str,
) -> Result<(CompareCopy, Option<String>), String> {
    let problem = |e: aura_exec_isolation::IsolationError| format!("preparing the copy: {e}");
    let (copy, note) = if let Some(repo) = repo {
        let ws = WorktreeIsolation::new(dir)
            .provision(repo, name)
            .map_err(problem)?;
        let note = changes::carry_changes(repo, &ws.root, BUILD_OUTPUT)
            .err()
            .map(|e| {
                format!("{name} ran on HEAD: your uncommitted changes did not carry over ({e})")
            });
        let inside = workspace
            .strip_prefix(repo)
            .unwrap_or_else(|_| Path::new(""));
        let workdir = ws.root.join(inside);
        std::fs::create_dir_all(&workdir).map_err(|e| format!("preparing the copy: {e}"))?;
        let copy = CompareCopy {
            root: ws.root,
            workdir,
            git_dir: None,
            baseline: String::new(),
        };
        (copy, note)
    } else {
        let ws = CopyIsolation::new(dir)
            .skipping(BUILD_OUTPUT.iter().copied())
            .provision(workspace, name)
            .map_err(problem)?;
        let copy = CompareCopy {
            workdir: ws.root.clone(),
            root: ws.root,
            git_dir: Some(dir.join(format!("{name}.git"))),
            baseline: String::new(),
        };
        (copy, None)
    };
    if let Some(plan_file) = plan_file {
        let target = copy.workdir.join(plan_file);
        if !target.exists() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("copying the plan: {e}"))?;
            }
            std::fs::copy(workspace.join(plan_file), &target)
                .map_err(|e| format!("copying the plan: {e}"))?;
        }
    }
    let baseline = changes::baseline(&copy.root, copy.git_dir.as_deref()).map_err(problem)?;
    Ok((CompareCopy { baseline, ..copy }, note))
}

/// Run `n` of `arm` from its copy to its check, and hand its agent back.
async fn run_one(ctx: Arc<Context>, arm: CompareArm, n: u32) -> Ran {
    let name = run_name(arm, n);
    let (dir, workspace, repo) = (
        ctx.dir.clone(),
        ctx.plan.workspace.clone(),
        ctx.repo.clone(),
    );
    let (plan_file, copy_name) = (ctx.plan.plan.clone(), name.clone());
    let prepared = tokio::task::spawn_blocking(move || {
        prepare_copy(
            &dir,
            &workspace,
            repo.as_deref(),
            plan_file.as_deref(),
            &copy_name,
        )
    })
    .await
    .map_err(|e| format!("preparing the copy: {e}"))
    .and_then(|prepared| prepared);
    let (copy, note) = match prepared {
        Ok(prepared) => prepared,
        Err(problem) => return not_run(&ctx, arm, n, problem),
    };
    let hub = &ctx.hub;
    let registered =
        lock(&hub.registry).create_child(&name, &ctx.host.zns_id, Some(copy.workdir.clone()));
    let agent = match registered {
        Ok(agent) => agent,
        Err(e) => return not_run(&ctx, arm, n, format!("registering the run: {e:#}")),
    };
    let session = match hub
        .factory
        .open(&agent, Vec::new(), Some(copy.workdir.clone()))
    {
        Ok(session) => session,
        Err(e) => return not_run(&ctx, arm, n, format!("opening the run: {e:#}")),
    };
    let mut ran = run_turn(&ctx, session, arm, &name, copy).await;
    ran.order = (n, arm);
    ran.note = note;
    ran
}

/// Run the turn of the run `name` on `session` and measure it.
async fn run_turn(
    ctx: &Context,
    mut session: AgentSession,
    arm: CompareArm,
    name: &str,
    copy: CompareCopy,
) -> Ran {
    let hub = &ctx.hub;
    let plan = &ctx.plan;
    let agent_id = session.agent_id();
    // Nobody is there to approve the run's calls, and it works in its own copy.
    hub.set_mode(agent_id, PermissionMode::Unrestricted);
    super::modes::apply(
        &session.kernel,
        PermissionMode::Unrestricted,
        &hub.tool_names(),
    );
    let gate_routed = |delegate| ArmRouting::Gate {
        candidates: plan.candidates.clone(),
        delegate,
    };
    let routing = match (arm, &plan.against) {
        (CompareArm::Gate, _) => gate_routed(plan.delegate),
        (CompareArm::Other, Opponent::GateNoDelegate) => gate_routed(false),
        (CompareArm::Other, Opponent::Model(model)) => ArmRouting::Pinned(model.clone()),
    };
    hub.begin_compare_run(agent_id, routing.clone());
    let workdir = session.kernel.agent_workspace();
    let (mut config, tools) = hub.lead_turn(&mut session);
    config.observers.clear();
    if let Some(path) = plan.plan.as_deref().filter(|path| is_plan_path(path)) {
        if let Ok(plan_file) = PlanFile::load(&workdir.join(path)) {
            session.todos = todo_items(&plan_file);
            session.active_plan = Some(path.to_string());
            session.building_plan = true;
        }
    }
    config.initial_todos.clone_from(&session.todos);

    let cancel = ctx.cancel.child_token();
    let ui = AgentUi::new(ctx.commands.clone(), agent_id);
    let category = if plan.classifier.is_empty() {
        PromptCategory::Unclassified
    } else {
        gate_record::classify_prompt(
            &session.kernel,
            &plan.classifier,
            &plan.task,
            config.auth_token.clone(),
        )
        .await
    };
    let routed_to = match routing {
        ArmRouting::Gate { .. } => hub
            .gate
            .redecide(RouteLevel::Turn, category, &plan.candidates)
            .map(|decision| decision.model.id),
        ArmRouting::Pinned(model) => Some(model),
    };
    if let Some(model) = routed_to.filter(|model| *model != config.model) {
        hub.model_limits().configure(&mut config, &model);
    }
    let model = config.model.clone();
    let prompt = PromptPayload {
        category,
        classifier: plan.classifier.clone(),
        route: Route::turn(model.clone(), RouteReason::Compare),
        ..PromptPayload::new(plan.task.clone())
    };

    hub.register_root(agent_id, name, workdir.clone(), &config);
    let mut row = turn_summary(agent_id, &plan.task, &model);
    row.label = format!("{} {name}: {}", ctx.host.name, row.label);
    hub.processes.start(row, Some(cancel.clone()));
    hub.start_running(agent_id, Arc::clone(&session.kernel), cancel.clone(), None);
    hub.send_agent_list().await;
    ui.send(UiCommand::TurnStarted {
        prompt: Some(plan.task.clone()),
    })
    .await;
    ui.send(system_message(format!(
        "{} {name} · {} → {} · {category}",
        ctx.host.name,
        arm,
        aura_model_reasoner::model_catalog::route_label(&model)
    )))
    .await;

    let started = Instant::now();
    let prompt_tx = match record_prompt(&session, &prompt).await {
        Ok((tx, _)) => Some(tx.hash),
        Err(problem) => {
            warn!(%agent_id, %problem, "Failed to record a compare run's prompt");
            None
        }
    };
    let turn_start = session.messages.len();
    session.messages.push(Message::user(plan.task.clone()));
    let agent_loop = AgentLoop::new(config);
    let steering = Steering::new(&cancel);
    let (result, forwarded) = run_agent_turn(
        &session,
        &agent_loop,
        tools,
        &ui,
        &cancel,
        &steering,
        hub.long_run,
        None,
    )
    .await;
    steering.finish(&[]);
    let wall_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let cancelled = cancel.is_cancelled();
    let signal = if cancelled || forwarded.credentials_rejected {
        OutcomeSignal::None
    } else {
        ended_signal(&result, turn_start)
    };
    let (reply, mut error) = match result {
        Ok(result) => {
            let text = result.total_text.clone();
            let llm_error = result.llm_error.clone().filter(|_| text.trim().is_empty());
            handle_agent_success(&mut session, result, &forwarded, &ui, &[]).await;
            (text, llm_error)
        }
        Err(e) => {
            ui.send(UiCommand::ShowWarning(e.to_string())).await;
            ui.send(UiCommand::Complete).await;
            (String::new(), Some(e.to_string()))
        }
    };
    if cancelled {
        error = Some("stopped by the user".to_string());
    }
    let ended = prompt_tx.map(|prompt_tx| OutcomePayload {
        prompt_tx,
        category,
        model: model.clone(),
        success: signal.success(),
        signal,
        cost_usd: forwarded.usage.cost_usd,
        latency_ms: wall_ms,
    });
    if let Some(ended) = &ended {
        gate_record::record(&session.kernel, agent_id, ended).await;
    }

    let check_passed = match plan.check.as_deref() {
        Some(check) if !cancelled => {
            let run = task_check::run_script(&session, check, CHECK_SOURCE).await;
            ui.send(system_message(run.describe())).await;
            if let (Some(signal), Some(ended)) = (run.signal(), &ended) {
                gate_record::record(&session.kernel, agent_id, &ended.with_signal(signal)).await;
            }
            run.passed
        }
        _ => None,
    };

    let (root, git_dir, baseline) = (
        copy.root.clone(),
        copy.git_dir.clone(),
        copy.baseline.clone(),
    );
    let patch = tokio::task::spawn_blocking(move || {
        changes::diff_since(&root, git_dir.as_deref(), &baseline)
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|diff| diff.map_err(|e| e.to_string()))
    .unwrap_or_else(|problem| {
        warn!(%agent_id, %problem, "Could not diff a compare run");
        String::new()
    });
    let tokens = measure(hub, agent_id);
    let cost_usd = tokens.iter().map(|row| row.cost_usd).sum();

    hub.processes.finish(
        &turn_id(agent_id),
        if error.is_some() {
            ProcessStatus::Failed
        } else {
            ProcessStatus::Exited
        },
    );
    hub.end_compare_run(agent_id);
    hub.park(session);
    hub.send_agent_list().await;

    Ran {
        run: CompareRun {
            name: name.to_string(),
            arm,
            agent_id,
            prompt_tx,
            model,
            files_changed: u32::try_from(changes::files_in(&patch)).unwrap_or(u32::MAX),
            copy,
            wall_ms,
            cost_usd,
            tokens,
            check_passed,
            error,
        },
        order: (0, arm),
        reply,
        patch,
        note: None,
    }
}

/// Tokens and cost per model of every model call `agent` and the agents
/// under it made.
fn measure(hub: &AgentHub, agent: AgentId) -> Vec<ModelTokens> {
    let agents: Vec<AgentId> = std::iter::once(agent)
        .chain(lock(&hub.registry).descendants(agent))
        .collect();
    let store = hub.factory.store.as_ref();
    tally(
        agents
            .into_iter()
            .flat_map(|agent| model_calls(store, agent)),
    )
}

fn model_calls(store: &dyn ReadStore, agent: AgentId) -> Vec<(String, Usage)> {
    let mut calls = Vec::new();
    let mut from = 1;
    loop {
        let entries = match store.scan_record(agent, from, SCAN_BATCH) {
            Ok(entries) => entries,
            Err(e) => {
                warn!(%agent, error = %e, "Could not read a compare run's record");
                break;
            }
        };
        calls.extend(
            entries
                .iter()
                .filter_map(|entry| crate::record_loader::model_call(&entry.tx)),
        );
        match entries.last() {
            Some(last) if entries.len() == SCAN_BATCH => from = last.seq + 1,
            _ => break,
        }
    }
    calls
}

/// A number from `agent` to seed the judge's order with.
fn seed(agent: AgentId) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&agent.0[..8]);
    u64::from_le_bytes(bytes)
}

/// Have the judge compare each Gate run with the other arm's run of the
/// same number, through the comparison's own kernel.
async fn judge_all(ctx: &Context, session: &AgentSession, ran: &[Ran]) -> Vec<CompareJudgment> {
    let finished = |arm: CompareArm| -> Vec<&Ran> {
        ran.iter()
            .filter(|r| r.run.arm == arm && r.run.error.is_none())
            .collect()
    };
    let pairs: Vec<(&Ran, &Ran)> = finished(CompareArm::Gate)
        .into_iter()
        .zip(finished(CompareArm::Other))
        .collect();
    if pairs.is_empty() || ctx.plan.judge.is_empty() {
        return Vec::new();
    }
    let auth_token = ctx.hub.turn(None, &ctx.plan.workspace).0.auth_token;
    let seed = seed(ctx.host.agent_id());
    let attempt = |r: &Ran| judge::Attempt {
        diff: r.patch.clone(),
        reply: r.reply.clone(),
    };
    let mut judgments = Vec::with_capacity(pairs.len());
    for (i, (gate, other)) in pairs.into_iter().enumerate() {
        if ctx.cancel.is_cancelled() {
            break;
        }
        let gate_first = judge::gate_first(seed, i);
        let mut judgment = CompareJudgment {
            gate_run: gate.run.name.clone(),
            other_run: other.run.name.clone(),
            gate_first,
            winner: None,
            parsed: false,
            model: ctx.plan.judge.clone(),
            cost_usd: 0.0,
            reasoning: String::new(),
        };
        let request = judge::request(
            &ctx.plan.judge,
            &ctx.plan.task,
            &attempt(gate),
            &attempt(other),
            gate_first,
            auth_token.clone(),
        );
        let reply = match request {
            Ok(request) => {
                match tokio::time::timeout(JUDGE_TIMEOUT, session.kernel.reason(request)).await {
                    Ok(Ok(result)) => {
                        judgment.cost_usd =
                            list_price(&ctx.plan.judge, &result.response.usage).unwrap_or(0.0);
                        Ok(judge::reply_text(&result.response))
                    }
                    Ok(Err(e)) => Err(format!("judge call failed: {e}")),
                    Err(_) => Err("judge call timed out".to_string()),
                }
            }
            Err(e) => Err(format!("judge request: {e}")),
        };
        match reply {
            Ok(text) => {
                if let Some(verdict) = judge::parse(&text) {
                    judgment.parsed = true;
                    judgment.winner = judge::winner(verdict, gate_first);
                }
                judgment.reasoning = text.chars().take(MAX_REASONING).collect();
            }
            Err(problem) => {
                warn!(%problem, "Judge gave no verdict");
                judgment.reasoning = problem;
            }
        }
        judgments.push(judgment);
    }
    judgments
}

/// Remove the copies of `payload`'s runs and the folder they were in.
/// Returns what could not be removed.
pub(super) fn remove_copies(payload: &ComparePayload) -> Vec<String> {
    let mut problems = Vec::new();
    let mut parents: Vec<PathBuf> = Vec::new();
    for run in &payload.runs {
        let copy = &run.copy;
        if copy.root.as_os_str().is_empty() {
            continue;
        }
        let Some(parent) = copy.root.parent().map(Path::to_path_buf) else {
            continue;
        };
        let workspace = IsolatedWorkspace {
            root: copy.root.clone(),
            strategy: if copy.git_dir.is_some() {
                IsolationStrategy::Copy
            } else {
                IsolationStrategy::Worktree
            },
        };
        let removed = if copy.git_dir.is_some() {
            CopyIsolation::new(&parent).teardown(&workspace)
        } else {
            WorktreeIsolation::new(&parent).teardown(&workspace)
        };
        if let Err(e) = removed {
            problems.push(format!("{}: {e}", run.name));
        }
        if let Some(git_dir) = copy.git_dir.as_deref().filter(|dir| dir.exists()) {
            if let Err(e) = std::fs::remove_dir_all(git_dir) {
                problems.push(format!("{}: {e}", git_dir.display()));
            }
        }
        if copy.root.exists() {
            problems.push(format!("{} is still there", copy.root.display()));
        }
        if !parents.contains(&parent) {
            parents.push(parent);
        }
    }
    for parent in parents {
        // Only once it is empty; anything else in it is not ours.
        let _ = std::fs::remove_dir(parent);
    }
    problems
}

/// What each run of `payload` changed since its copy was made, by run
/// name; runs whose copy is gone or can't be read are left out.
pub(super) fn patches(payload: &ComparePayload) -> HashMap<String, Result<String, String>> {
    payload
        .runs
        .iter()
        .filter(|run| !run.copy.root.as_os_str().is_empty())
        .map(|run| {
            let copy = &run.copy;
            let patch = if copy.root.exists() {
                changes::diff_since(&copy.root, copy.git_dir.as_deref(), &copy.baseline)
                    .map_err(|e| e.to_string())
            } else {
                Err(format!("the copy of {} was removed", run.name))
            };
            (run.name.clone(), patch)
        })
        .collect()
}
