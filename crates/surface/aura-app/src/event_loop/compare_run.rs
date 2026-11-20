//! Running a `/compare` (a benchmark): every run of both arms in its own
//! copy of the workspace, then the blind judge, then the record and the
//! view.
//!
//! Each run is an agent of its own, registered under the comparison's
//! agent with the copy as its folder, so it shows in the Agents panel
//! and the Processes tab while it works, its subtasks work in the same
//! copy, and its records feed Gate's table like any turn. Its turn runs
//! on the same turn driver as the terminal's turns and `task` spawns:
//!
//! - a Gate arm's turns go where Gate sends them with exploration off,
//!   and their subtasks are routed the same way;
//! - a pinned arm's turns and subtasks run on the model it is pinned to.
//!
//! A run sends its prompts one after the other on the same conversation,
//! as a replayed session did. Each prompt is recorded with route reason
//! `compare` and its turn ends with its `Outcome`; what each turn cost is
//! kept per prompt. After the last turn the check command runs in the copy as a
//! `run_command` call through the run's kernel, so it is in the run's log,
//! and its verdict is recorded as a later `Outcome`, so it counts in the
//! table.
//! Cost and tokens per model come from the records of the run and its
//! subtasks: every model call there, classifier calls included. So do
//! the plan readings those calls recorded; once every run has ended they
//! are shared out between the runs with `plan_shares`.

use super::agent_events::AgentUi;
use super::agents::AgentSession;
use super::bench_source::{self, StartFile};
use super::compare_view::{duration, money, render};
use super::gate_record;
use super::handlers::{handle_agent_success, run_agent_turn, system_message, turn_summary};
use super::modes::Permissions;
use super::processes::{now_ms, turn_id};
use super::steering::{SteerSource, Steering};
use super::subagents::{lock, AgentHub, ArmRouting};
use super::task_check::{self, takeover_prompt};
use super::tasks::TASKS_DIR;
use super::turns::{ended_signal, record_prompt};
use crate::agent_registry::RegisteredAgent;
use crate::bench_pack::{self, ExampleTask};
use crate::isolation::changes::{self, BUILD_OUTPUT};
use crate::isolation::{
    CopyIsolation, IsolatedWorkspace, Isolation, IsolationStrategy, WorktreeIsolation,
};
use aura_agent::AgentLoop;
use aura_agent_gate::classify::Label;
use aura_agent_gate::compare::{list_price, plan_shares, run_name, PlanReading, Schedule};
use aura_agent_gate::outcome::calls_for_stronger;
use aura_agent_gate::{judge, Candidate, Work};
use aura_agent_kernel::Kernel;
use aura_core_types::{
    AgentId, ArmSpec, BenchSource, CompareArm, CompareCopy, CompareJudgment, ComparePayload,
    CompareRun, CompareStatus, Hash, OutcomePayload, OutcomeSignal, PromptPayload, Route,
    RouteLevel, RouteReason, RunTurn, SystemKind, Transaction,
};
use aura_exec_tools::plan_tools::{is_plan_path, todo_items, PlanFile};
use aura_model::Message;
use aura_ui_terminal::events::ProcessStatus;
use aura_ui_terminal::{ApprovalLevel, PermissionMode, UiCommand};
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

/// A comparison as the user confirmed it.
#[derive(Debug, Clone)]
pub(super) struct ComparePlan {
    /// What it is about: its prompt, or a replayed session's first one
    pub task: String,
    /// The prompts each run sends in order (the build prompt for a plan);
    /// empty when the work is example tasks
    pub prompts: Vec<String>,
    /// Example tasks: run `n` of either arm works on task
    /// `(n - 1) % examples.len()`, in a copy of its own files, with its
    /// prompts and its check
    pub examples: Vec<&'static ExampleTask>,
    /// Plan file each run builds, relative to the workspace
    pub plan: Option<String>,
    /// What arm A and arm B run on
    pub arms: [ArmSpec; 2],
    /// Where the work comes from
    pub source: BenchSource,
    /// Files put in each copy before its run: a replayed session's files
    /// as they were when it started
    pub start: Arc<Vec<StartFile>>,
    /// What setting it up found out, for the record
    pub notes: Vec<String>,
    /// Model that judges
    pub judge: String,
    pub repeats: u32,
    pub check: Option<String>,
    pub budget_usd: Option<f64>,
    /// What Gate routes a Gate arm among
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
    /// Whether Gate arms explore, to learn
    pub learn: bool,
}

impl ComparePlan {
    /// Runs each arm gets: `repeats` of each example task, else `repeats`.
    pub(super) fn runs_per_arm(&self) -> u32 {
        let tasks = u32::try_from(self.examples.len().max(1)).unwrap_or(u32::MAX);
        self.repeats.saturating_mul(tasks)
    }

    /// The example task run `n` works on, if the work is example tasks.
    fn example(&self, n: u32) -> Option<&'static ExampleTask> {
        let i = usize::try_from(n.max(1) - 1).ok()?;
        (!self.examples.is_empty()).then(|| self.examples[i % self.examples.len()])
    }
}

/// Folder of a comparison's copies that holds the example tasks' files
/// the copies are made from.
const FIXTURES: &str = "fixtures";

/// Where `task`'s files are kept in the comparison's folder `dir`.
fn fixture(dir: &Path, task: &ExampleTask) -> PathBuf {
    dir.join(FIXTURES).join(task.id.replace('/', "__"))
}

/// What the agent grouping `arm`'s runs, on `spec`, is called:
/// `A · gate`, `B · claude-opus-5-5`.
fn arm_name(arm: CompareArm, spec: &ArmSpec) -> String {
    let label = spec.label();
    let short = label.rsplit('/').next().unwrap_or(label);
    format!("{} · {short}", arm.as_str().to_uppercase())
}

/// The task records a run's subtasks left in its copy, moved out of it
/// while its check runs: they quote the prompts, so a check that greps the
/// copy would find them.
struct RecordsAside {
    /// Where they were, and where they are meanwhile
    moved: Option<(PathBuf, PathBuf)>,
}

impl RecordsAside {
    /// Move the records under `workdir` beside the copy at `root`, which
    /// the run `name` works in.
    fn take(workdir: &Path, root: &Path, name: &str) -> Self {
        let records = workdir.join(TASKS_DIR);
        let aside = root.with_file_name(format!("{name}.tasks"));
        let moved = records.is_dir() && !aside.exists();
        let moved = match moved.then(|| std::fs::rename(&records, &aside)) {
            Some(Ok(())) => Some((records, aside)),
            Some(Err(e)) => {
                warn!(run = name, error = %e, "Could not move the task records out of the check's way");
                None
            }
            None => None,
        };
        Self { moved }
    }

    fn put_back(self) {
        if let Some((records, aside)) = self.moved {
            if let Err(e) = std::fs::rename(&aside, &records) {
                warn!(error = %e, from = %aside.display(), "Could not put the task records back");
            }
        }
    }
}

/// What the runs of one comparison share.
struct Context {
    hub: Arc<AgentHub>,
    commands: mpsc::Sender<UiCommand>,
    plan: ComparePlan,
    host: RegisteredAgent,
    /// ZNS ids of the agents each arm's runs are registered under, A first
    arms: [String; 2],
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
    /// Plan usage its model calls reported
    readings: Vec<PlanReading>,
}

/// Write `payload` to the record of the comparison's agent `agent_id`
/// (named `name`) and show it in the BENCH tab. A failure to record is
/// logged.
pub(super) async fn save(
    hub: &AgentHub,
    kernel: &Kernel,
    commands: &mpsc::Sender<UiCommand>,
    (agent_id, name): (AgentId, &str),
    payload: &ComparePayload,
) -> bool {
    let recorded = match kernel
        .process_direct(Transaction::compare(agent_id, payload))
        .await
    {
        Ok(_) => true,
        Err(e) => {
            warn!(%agent_id, error = %e, "Failed to record the comparison");
            false
        }
    };
    let root = lock(&hub.registry).root(agent_id);
    let view = super::bench::view(agent_id, root, name, payload);
    let _ = commands.send(UiCommand::BenchUpsert(Box::new(view))).await;
    recorded
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
        arm_a: plan.arms[0].clone(),
        arm_b: plan.arms[1].clone(),
        against: plan.arms[1].label().to_string(),
        prompts: plan.prompts.clone(),
        source: plan.source.clone(),
        repeats: plan.repeats,
        check: plan.check.clone(),
        budget_usd: plan.budget_usd,
        requested_by: plan.user,
        apply_root: repo.clone().unwrap_or_else(|| plan.workspace.clone()),
        started_ms: now_ms(),
        runs: Vec::new(),
        judgments: Vec::new(),
        notes: plan.notes.clone(),
        status: CompareStatus::Running,
    };
    if plan.examples.is_empty() && dir.starts_with(&payload.apply_root) {
        payload.notes.push(format!(
            "The copies would land inside {}, the folder they copy (AURA_HOME is in it); \
             set AURA_HOME elsewhere to compare here",
            payload.apply_root.display()
        ));
        payload.status = CompareStatus::Cancelled;
        finish(&hub, &commands, &user_ui, &host, session, &payload).await;
        return;
    }
    let cache = crate::repo_source::cache_dir();
    let unfetched = plan
        .examples
        .iter()
        .filter_map(|task| task.source.as_ref())
        .filter(|source| source.dir(&cache).is_ok_and(|dir| !dir.is_dir()))
        .count();
    if unfetched > 0 {
        payload.notes.push(format!(
            "{unfetched} task{} fetched before the runs start; pulling a task's container image can take minutes",
            if unfetched == 1 { "'s repository is" } else { "s' repositories are" }
        ));
    }
    save(
        &hub,
        &session.kernel,
        &commands,
        (host_id, &host.name),
        &payload,
    )
    .await;
    if !plan.examples.is_empty() {
        let mut written = Ok(());
        for task in &plan.examples {
            if let Err(e) = task.materialize(&fixture(&dir, task), &cache).await {
                written = Err(format!("writing the files of {}: {e}", task.id));
                break;
            }
        }
        if let Err(problem) = written {
            payload.notes.push(problem);
            payload.status = CompareStatus::Cancelled;
            finish(&hub, &commands, &user_ui, &host, session, &payload).await;
            return;
        }
    }

    let arms = CompareArm::BOTH.map(|arm| {
        let name = arm_name(arm, &plan.arms[arm.index()]);
        match lock(&hub.registry).create_compare_arm(&name, &host.zns_id) {
            Ok(agent) => agent.zns_id,
            Err(e) => {
                warn!(arm = %arm, error = %e, "Could not register an arm; its runs go under the comparison");
                host.zns_id.clone()
            }
        }
    });
    let ctx = Arc::new(Context {
        hub: Arc::clone(&hub),
        commands,
        plan,
        host: host.clone(),
        arms,
        dir,
        repo,
        cancel,
    });
    let mut ran = run_all(&ctx, &session, &user_ui, &mut payload).await;
    ran.sort_by_key(|r| r.order);
    let readings: Vec<&[PlanReading]> = ran.iter().map(|r| r.readings.as_slice()).collect();
    let shares = plan_shares(&readings);
    for (r, share) in ran.iter_mut().zip(shares) {
        r.run.plan_usage = share;
    }
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
    finish(&hub, &ctx.commands, &user_ui, &host, session, &payload).await;
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
    save(
        hub,
        &session.kernel,
        commands,
        (host.agent_id(), &host.name),
        payload,
    )
    .await;
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
    let mut schedule = Schedule::new(plan.runs_per_arm(), plan.concurrency, plan.budget_usd);
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
        save(
            &ctx.hub,
            &session.kernel,
            &ctx.commands,
            (ctx.host.agent_id(), &ctx.host.name),
            payload,
        )
        .await;
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
            plan_usage: Vec::new(),
            turns: Vec::new(),
            check_passed: None,
            files_changed: 0,
            error: Some(error),
            task: ctx.plan.example(n).map(|task| task.id.clone()),
        },
        order: (n, arm),
        reply: String::new(),
        patch: String::new(),
        note: None,
        readings: Vec::new(),
    }
}

/// Make the copy run `name` works in: a detached worktree of the user's
/// repository with their uncommitted work carried over, else a copy of
/// their folder without build output, with `start` put in it. Returns it
/// with a note when the uncommitted work could not be carried.
fn prepare_copy(
    dir: &Path,
    workspace: &Path,
    repo: Option<&Path>,
    plan_file: Option<&str>,
    start: &[StartFile],
    name: &str,
) -> Result<(CompareCopy, Option<String>), String> {
    let problem = |e: crate::isolation::IsolationError| format!("preparing the copy: {e}");
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
    bench_source::restore(&copy.workdir, start)?;
    let baseline = changes::baseline(&copy.root, copy.git_dir.as_deref()).map_err(problem)?;
    Ok((CompareCopy { baseline, ..copy }, note))
}

/// Run `n` of `arm` from its copy to its check, and hand its agent back.
async fn run_one(ctx: Arc<Context>, arm: CompareArm, n: u32) -> Ran {
    let name = run_name(arm, n);
    let example = ctx.plan.example(n);
    let (dir, workspace, repo) = match example {
        Some(task) => (ctx.dir.clone(), fixture(&ctx.dir, task), None),
        None => (
            ctx.dir.clone(),
            ctx.plan.workspace.clone(),
            ctx.repo.clone(),
        ),
    };
    let (plan_file, start, copy_name) = (
        ctx.plan.plan.clone(),
        Arc::clone(&ctx.plan.start),
        name.clone(),
    );
    let prepared = tokio::task::spawn_blocking(move || {
        prepare_copy(
            &dir,
            &workspace,
            repo.as_deref(),
            plan_file.as_deref(),
            &start,
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
        lock(&hub.registry).create_child(&name, &ctx.arms[arm.index()], Some(copy.workdir.clone()));
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
    let mut ran = run_turn(&ctx, session, (arm, &name, example), copy).await;
    ran.order = (n, arm);
    ran.note = note;
    ran
}

/// One turn of a run: what its prompt got.
struct Turned {
    prompt_tx: Option<Hash>,
    model: String,
    signal: OutcomeSignal,
    reply: String,
    error: Option<String>,
    wall_ms: u64,
    /// Its outcome as recorded, for the check's verdict to follow
    ended: Option<OutcomePayload>,
    /// How it was routed and what its subtasks cost, for the run's
    /// record; its model, cost, time and signal are filled in there
    routed: RunTurn,
}

/// What `agent`'s calls and its subtasks' have cost so far.
fn spent(hub: &AgentHub, agent: AgentId) -> f64 {
    hub.tokens_by_model(agent)
        .iter()
        .map(|row| row.cost_usd)
        .sum()
}

/// Run the prompts of the run `name` (those of `example`, when it works
/// on one) on `session` one after the other, then its check, and measure
/// it.
async fn run_turn(
    ctx: &Context,
    mut session: AgentSession,
    (arm, name, example): (CompareArm, &str, Option<&'static ExampleTask>),
    copy: CompareCopy,
) -> Ran {
    let hub = &ctx.hub;
    let plan = &ctx.plan;
    let agent_id = session.agent_id();
    // The run is unattended and works in its own copy, so it runs at yolo:
    // nothing asks, and its commands may only write inside the copy.
    let permissions = Permissions::new(PermissionMode::Agent, ApprovalLevel::Yolo);
    hub.set_permissions(agent_id, permissions);
    super::modes::apply(&session.kernel, permissions, &hub.tool_names());
    let spec = &plan.arms[arm.index()];
    let routing = match spec {
        ArmSpec::Gate { delegate } => ArmRouting::Gate {
            candidates: plan.candidates.clone(),
            delegate: *delegate,
        },
        ArmSpec::Model { model } => ArmRouting::Pinned(model.clone()),
    };
    hub.begin_compare_run(agent_id, routing.clone());
    let workdir = session.kernel.agent_workspace();
    if let Some(path) = plan.plan.as_deref().filter(|path| is_plan_path(path)) {
        if let Ok(plan_file) = PlanFile::load(&workdir.join(path)) {
            session.todos = todo_items(&plan_file);
            session.active_plan = Some(path.to_string());
            session.building_plan = true;
        }
    }

    let cancel = ctx.cancel.child_token();
    let ui = AgentUi::new(ctx.commands.clone(), agent_id);
    let task = example.map_or(plan.task.as_str(), |task| task.title.as_str());
    let mut row = turn_summary(agent_id, task, spec.label());
    row.label = format!("{} {name}: {}", ctx.host.name, row.label);
    hub.processes.start(row, Some(cancel.clone()));
    hub.start_running(agent_id, Arc::clone(&session.kernel), cancel.clone(), None);

    let prompts = example.map_or(plan.prompts.as_slice(), |task| task.prompts.as_slice());
    let started = Instant::now();
    let mut turns = Vec::with_capacity(prompts.len());
    let mut first: Option<(Option<Hash>, String)> = None;
    let (mut reply, mut error, mut last) = (String::new(), None, None);
    for (i, prompt) in prompts.iter().enumerate() {
        if cancel.is_cancelled() {
            error = Some("stopped by the user".to_string());
            break;
        }
        let before = spent(hub, agent_id);
        let turned = run_prompt(
            ctx,
            &mut session,
            (arm, name),
            &routing,
            (i, prompts.len(), prompt),
            &cancel,
            &ui,
        )
        .await;
        turns.push(RunTurn {
            model: turned.model.clone(),
            cost_usd: (spent(hub, agent_id) - before).max(0.0),
            wall_ms: turned.wall_ms,
            signal: turned.signal,
            ..turned.routed
        });
        first.get_or_insert_with(|| (turned.prompt_tx, turned.model.clone()));
        reply = turned.reply;
        last = turned.ended;
        if turned.error.is_some() {
            error = turned.error;
            break;
        }
    }
    let wall_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let (prompt_tx, model) = first.unwrap_or_default();

    let check = example.map_or(plan.check.as_deref(), |task| Some(task.verify.as_str()));
    let check_passed = match check {
        Some(check) if !cancel.is_cancelled() && !turns.is_empty() => {
            let hidden = match example {
                Some(task) => task
                    .place_checks(&workdir)
                    .map_err(|e| format!("putting the hidden checks of {} in place: {e}", task.id)),
                None => Ok(Vec::new()),
            };
            match hidden {
                Ok(hidden) => {
                    let records = RecordsAside::take(&workdir, &copy.root, name);
                    let run = task_check::run_script(&session, check, CHECK_SOURCE).await;
                    records.put_back();
                    bench_pack::remove_checks(&hidden);
                    ui.send(system_message(run.describe())).await;
                    // A run its provider cut short did not finish the work
                    // the check looks at.
                    let finished = last
                        .as_ref()
                        .filter(|ended| ended.signal != OutcomeSignal::Unavailable);
                    if let (Some(signal), Some(ended)) = (run.signal(), finished) {
                        gate_record::record(&session.kernel, agent_id, &ended.with_signal(signal))
                            .await;
                    }
                    run.passed
                }
                Err(problem) => {
                    warn!(%agent_id, %problem, "Could not check an example run");
                    ui.send(UiCommand::ShowWarning(problem)).await;
                    None
                }
            }
        }
        _ => None,
    };

    let (root, git_dir, baseline) = (
        copy.root.clone(),
        copy.git_dir.clone(),
        copy.baseline.clone(),
    );
    let patch = tokio::task::spawn_blocking(move || {
        changes::diff_since(&root, git_dir.as_deref(), &baseline, &[TASKS_DIR])
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|diff| diff.map_err(|e| e.to_string()))
    .unwrap_or_else(|problem| {
        warn!(%agent_id, %problem, "Could not diff a compare run");
        String::new()
    });
    let tokens = hub.tokens_by_model(agent_id);
    let cost_usd = tokens.iter().map(|row| row.cost_usd).sum();
    let readings = hub.plan_readings(agent_id);

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
            plan_usage: Vec::new(),
            turns,
            check_passed,
            error,
            task: example.map(|task| task.id.clone()),
        },
        order: (0, arm),
        reply,
        patch,
        note: None,
        readings,
    }
}

/// Send prompt `i` of the `of` of the run `name` on `session` and run its
/// turn.
async fn run_prompt(
    ctx: &Context,
    session: &mut AgentSession,
    (arm, name): (CompareArm, &str),
    routing: &ArmRouting,
    (i, of, text): (usize, usize, &str),
    cancel: &CancellationToken,
    ui: &AgentUi,
) -> Turned {
    let hub = &ctx.hub;
    let plan = &ctx.plan;
    let agent_id = session.agent_id();
    let workdir = session.kernel.agent_workspace();
    let (mut config, mut tools, lead) = hub.user_turn(session);
    config.observers.clear();
    config.initial_todos.clone_from(&session.todos);
    let label = if plan.classifier.is_empty() {
        Label::UNCLASSIFIED
    } else {
        gate_record::classify_prompt(
            &session.kernel,
            &plan.classifier,
            text,
            config.auth_token.clone(),
        )
        .await
    };
    let category = label.category;
    let work = Work::turn(category).of(label.difficulty);
    let decision = match routing {
        ArmRouting::Gate { .. } if plan.learn => hub.gate.decide(&work, &plan.candidates),
        ArmRouting::Gate { .. } => hub.gate.redecide(&work, &plan.candidates),
        ArmRouting::Pinned(_) => None,
    };
    let routed_to = match routing {
        ArmRouting::Gate { .. } => decision.as_ref().map(|d| d.model.id.clone()),
        ArmRouting::Pinned(model) => Some(model.clone()),
    };
    if let Some(model) = routed_to.filter(|model| *model != config.model) {
        hub.model_limits().configure(&mut config, &model);
    }
    let model = config.model.clone();
    let led = lead.is_some_and(|lead| {
        hub.gate.note_model(agent_id, &model);
        let led = hub.gate.way(category, &model, false).led;
        session.lead = lead.apply(led, &mut config, &mut tools);
        led
    });
    let prompt = PromptPayload {
        category,
        classifier: plan.classifier.clone(),
        route: Route {
            explored: decision.as_ref().is_some_and(|d| d.explored),
            propensity: decision.as_ref().map_or(1.0, |d| d.propensity),
            ..Route::turn(model.clone(), RouteReason::Compare)
        },
        led: Some(led),
        difficulty: label.difficulty,
        ..PromptPayload::new(text.to_string())
    };

    if i == 0 {
        hub.register_root(agent_id, name, workdir, &config);
        hub.send_agent_list().await;
    }
    ui.send(UiCommand::TurnStarted {
        prompt: Some(text.to_string()),
    })
    .await;
    let step = if of > 1 {
        format!(" · prompt {} of {of}", i + 1)
    } else {
        String::new()
    };
    ui.send(system_message(format!(
        "{} {name} · {} → {} · {category}{step}",
        ctx.host.name,
        plan.arms[arm.index()].label(),
        aura_model::model_catalog::route_label(&model)
    )))
    .await;

    let started = Instant::now();
    let started_ms = super::processes::now_ms();
    let prompt_tx = match record_prompt(session, &prompt).await {
        Ok((tx, _)) => Some(tx.hash),
        Err(problem) => {
            warn!(%agent_id, %problem, "Failed to record a compare run's prompt");
            None
        }
    };
    let first_tx = prompt_tx;
    let (mut model, mut prompt_tx) = (model, prompt_tx);
    let (mut attempt_started, mut attempt_ms) = (started, started_ms);
    let mut escalations = 0;
    let mut turn_start = session.messages.len();
    session.messages.push(Message::user(text.to_string()));
    // A Gate run's failed attempt goes on in the same conversation on the
    // next stronger model, as a routed turn does; the run's check judges
    // the result, so no judge is asked first.
    let (signal, reply, mut error, ended) = loop {
        let agent_loop = AgentLoop::new(config.clone());
        let steering = Steering::new(cancel);
        let (result, forwarded) = run_agent_turn(
            session,
            &agent_loop,
            tools.clone(),
            ui,
            cancel,
            &steering,
            hub.long_run,
            None,
        )
        .await;
        steering.finish(&[]);
        let cancelled = cancel.is_cancelled();
        let signal = if cancelled || forwarded.credentials_rejected {
            OutcomeSignal::None
        } else {
            ended_signal(&result, turn_start)
        };
        let next = match routing {
            ArmRouting::Gate { .. } if result.is_ok() && calls_for_stronger(signal) => hub
                .gate
                .escalate(RouteLevel::Turn, category, &model, &plan.candidates),
            _ => None,
        };
        let (reply, error) = match result {
            Ok(result) => {
                let text = result.total_text.clone();
                let llm_error = result.llm_error.clone().filter(|_| text.trim().is_empty());
                handle_agent_success(session, result, &forwarded, ui, &[]).await;
                (text, llm_error)
            }
            Err(e) => {
                ui.send(UiCommand::ShowWarning(e.to_string())).await;
                ui.send(UiCommand::Complete).await;
                (String::new(), Some(e.to_string()))
            }
        };
        let ended = prompt_tx.map(|prompt_tx| OutcomePayload {
            prompt_tx,
            category,
            model: model.clone(),
            success: signal.success(),
            signal,
            cost_usd: forwarded.usage.cost_usd,
            subtask_cost_usd: hub.subtask_cost_usd(agent_id, attempt_ms),
            latency_ms: u64::try_from(attempt_started.elapsed().as_millis()).unwrap_or(u64::MAX),
        });
        let Some(next) = next else {
            break (signal, reply, error, ended);
        };
        if let Some(ended) = &ended {
            gate_record::record(&session.kernel, agent_id, ended).await;
        }
        let takeover = format!(
            "{}{}",
            SteerSource::Harness.prefix(),
            takeover_prompt(
                &aura_model::model_catalog::route_label(&model),
                signal,
                None
            )
        );
        let escalated = PromptPayload {
            category,
            classifier: plan.classifier.clone(),
            route: next.route(RouteLevel::Turn),
            ..PromptPayload::new(takeover.clone())
        };
        prompt_tx = match record_prompt(session, &escalated).await {
            Ok((tx, _)) => Some(tx.hash),
            Err(problem) => {
                warn!(%agent_id, %problem, "Failed to record a compare run's escalated prompt");
                None
            }
        };
        info!(%agent_id, from = %model, to = %next.model.id, %signal, "Gate moves the run's turn to a stronger model");
        ui.send(system_message(format!(
            "{} {name} · gate → {} · {category}, {}",
            ctx.host.name,
            aura_model::model_catalog::route_label(&next.model.id),
            next.reason
        )))
        .await;
        hub.model_limits().configure(&mut config, &next.model.id);
        hub.gate.note_model(agent_id, &next.model.id);
        model.clone_from(&next.model.id);
        escalations += 1;
        (attempt_started, attempt_ms) = (Instant::now(), super::processes::now_ms());
        turn_start = session.messages.len();
        session.messages.push(Message::user(takeover));
    };
    if cancel.is_cancelled() {
        error = Some("stopped by the user".to_string());
    }
    if let Some(ended) = &ended {
        gate_record::record(&session.kernel, agent_id, ended).await;
    }
    let routed = RunTurn {
        category: Some(category),
        difficulty: label.difficulty,
        led: Some(led),
        escalations,
        subtask_cost_usd: hub.subtask_cost_usd(agent_id, started_ms),
        ..RunTurn::new(model.clone(), 0.0, 0, signal)
    };
    Turned {
        prompt_tx: first_tx,
        model,
        signal,
        reply,
        error,
        wall_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        ended,
        routed,
    }
}

/// A number from `agent` to seed the judge's order with.
fn seed(agent: AgentId) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&agent.0[..8]);
    u64::from_le_bytes(bytes)
}

/// What the judge is told the runs were asked: the prompt, or every
/// prompt in order.
fn judge_task(prompts: &[String]) -> String {
    match prompts {
        [only] => only.clone(),
        _ => {
            let mut task = String::from("The user sent these messages in order:\n");
            for (i, prompt) in prompts.iter().enumerate() {
                task.push_str(&format!("\n### Message {}\n{prompt}\n", i + 1));
            }
            task
        }
    }
}

/// Have the judge compare each run of arm A with arm B's run of the same
/// number, through the comparison's own kernel, and rate each against
/// what the user kept when the work is a replayed session.
async fn judge_all(ctx: &Context, session: &AgentSession, ran: &[Ran]) -> Vec<CompareJudgment> {
    let finished = |arm: CompareArm| -> Vec<&Ran> {
        ran.iter()
            .filter(|r| r.run.arm == arm && r.run.error.is_none())
            .collect()
    };
    let pairs: Vec<(&Ran, &Ran)> = if ctx.plan.examples.is_empty() {
        finished(CompareArm::A)
            .into_iter()
            .zip(finished(CompareArm::B))
            .collect()
    } else {
        // Only runs of the same number worked on the same task.
        let b = finished(CompareArm::B);
        finished(CompareArm::A)
            .into_iter()
            .filter_map(|a| {
                let n = a.order.0;
                b.iter().find(|b| b.order.0 == n).map(|b| (a, *b))
            })
            .collect()
    };
    if pairs.is_empty() || ctx.plan.judge.is_empty() {
        return Vec::new();
    }
    let auth_token = ctx.hub.turn(None, &ctx.plan.workspace).0.auth_token;
    let seed = seed(ctx.host.agent_id());
    let shared_task = judge_task(&ctx.plan.prompts);
    let reference = ctx.plan.source.reference().map(|r| judge::Attempt {
        diff: r.diff.clone(),
        reply: r.reply.clone(),
    });
    let attempt = |r: &Ran| judge::Attempt {
        diff: r.patch.clone(),
        reply: r.reply.clone(),
    };
    let mut judgments = Vec::with_capacity(pairs.len());
    for (i, (a, b)) in pairs.into_iter().enumerate() {
        if ctx.cancel.is_cancelled() {
            break;
        }
        let a_first = judge::a_first(seed, i);
        let mut judgment = CompareJudgment {
            a_run: a.run.name.clone(),
            b_run: b.run.name.clone(),
            a_first,
            winner: None,
            parsed: false,
            a_vs_reference: None,
            b_vs_reference: None,
            model: ctx.plan.judge.clone(),
            cost_usd: 0.0,
            reasoning: String::new(),
        };
        let task = ctx
            .plan
            .example(a.order.0)
            .map_or_else(|| shared_task.clone(), |task| judge_task(&task.prompts));
        let request = judge::request(
            &ctx.plan.judge,
            &task,
            (&attempt(a), &attempt(b)),
            a_first,
            reference.as_ref(),
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
                    judgment.winner = judge::winner(verdict, a_first);
                }
                if reference.is_some() {
                    (judgment.a_vs_reference, judgment.b_vs_reference) =
                        judge::reference_verdicts(&text, a_first);
                }
                let named = judge::unblind(&text, a_first, (&a.run.name, &b.run.name));
                judgment.reasoning = named.chars().take(MAX_REASONING).collect();
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
        let fixtures = parent.join(FIXTURES);
        if !payload.source.examples().is_empty() && fixtures.is_dir() {
            if let Err(e) = std::fs::remove_dir_all(&fixtures) {
                problems.push(format!("{}: {e}", fixtures.display()));
            }
        }
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
                changes::diff_since(
                    &copy.root,
                    copy.git_dir.as_deref(),
                    &copy.baseline,
                    &[TASKS_DIR],
                )
                .map_err(|e| e.to_string())
            } else {
                Err(format!("the copy of {} was removed", run.name))
            };
            (run.name.clone(), patch)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_records_are_out_of_the_copy_while_its_check_runs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("a-3");
        let records = root.join(TASKS_DIR);
        std::fs::create_dir_all(&records).unwrap();
        std::fs::write(records.join("1.json"), "rename calc_total").unwrap();

        let aside = RecordsAside::take(&root, &root, "a-3");
        assert!(
            !records.exists(),
            "a check grepping the copy can't see them"
        );
        aside.put_back();
        assert_eq!(
            std::fs::read_to_string(records.join("1.json")).unwrap(),
            "rename calc_total"
        );
        assert!(!dir.path().join("a-3.tasks").exists());

        let none = tempfile::tempdir().unwrap();
        RecordsAside::take(none.path(), none.path(), "b-1").put_back();
        assert!(!none.path().join(TASKS_DIR).exists());
    }
}
