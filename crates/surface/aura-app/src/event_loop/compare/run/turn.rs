//! A run's turns: its session set up as its arm's, its prompts sent one
//! after the other on the same conversation, then its check, and what
//! the run cost and changed.

use super::arm::{arm_name, short_label};
use super::check::{check, Checked};
use super::context::{elapsed_ms, Context, Ran, RunSpec, STOPPED};
use super::copy::patch_of;
use super::prompt::{run_prompt, Prompt};
use crate::event_loop::agent_events::AgentUi;
use crate::event_loop::agents::AgentSession;
use crate::event_loop::gate::followup::Before;
use crate::event_loop::handlers::turn_summary;
use crate::event_loop::modes::{self, Permissions};
use crate::event_loop::processes::turn_id;
use crate::event_loop::subagents::{AgentHub, ArmRouting};
use crate::isolation::changes;
use aura_agent_trace::{RunMeta, RunRecorder, RunSource};
use aura_core_types::{
    AgentId, ArmSpec, CompareCopy, CompareRun, Hash, OutcomePayload, PromptCategory, RunTurn,
};
use aura_exec_tools::plan_tools::{is_plan_path, todo_items, PlanFile};
use aura_gate::live::GateLive;
use aura_gate::route::Decision;
use aura_ui_terminal::events::ProcessStatus;
use aura_ui_terminal::{ApprovalLevel, PermissionMode};
use std::sync::Arc;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

/// What the turns of one run share.
pub(super) struct RunParts<'a> {
    pub ctx: &'a Context,
    pub run: &'a RunSpec,
    /// How the run's turns and subtasks are routed
    pub routing: &'a ArmRouting,
    /// The run's arm's own view of Gate, which routes its turns and
    /// subtasks
    pub gate: &'a Arc<GateLive>,
    /// Stops the run, and is stopped with the comparison
    pub cancel: CancellationToken,
    /// The run's chat
    pub ui: AgentUi,
    /// The run's trace; `None` when tracing is off
    pub recorder: Option<Arc<RunRecorder>>,
}

impl RunParts<'_> {
    /// What the run's arm runs on.
    pub(super) fn spec(&self) -> &ArmSpec {
        &self.ctx.plan.arms[self.run.arm.index()]
    }

    /// Whether Gate routes the run's turns, and follows them up.
    pub(super) fn gate_arm(&self) -> bool {
        matches!(self.routing, ArmRouting::Gate { .. })
    }

    /// The run's chat line saying Gate sent its turn asking for
    /// `category` on to `next`.
    pub(super) fn gate_line(&self, next: &Decision, category: PromptCategory) -> String {
        format!(
            "{} {} · gate → {} · {category}, {}",
            self.ctx.host.name,
            self.run.name,
            next.label(),
            next.reason
        )
    }
}

/// What a run's prompts left.
#[derive(Default)]
struct Prompted {
    turns: Vec<RunTurn>,
    /// The first prompt's recorded transaction, and the model it went to
    prompt_tx: Option<Hash>,
    model: String,
    /// The last turn's reply
    reply: String,
    /// Why the run stopped before its last prompt's turn ended well
    error: Option<String>,
    /// The last turn's `Outcome`, unless its provider cut it short: the
    /// work the check looks at was never finished then
    last: Option<OutcomePayload>,
    wall_ms: u64,
}

/// Run the prompts of `run` (those of its example task, when it works on
/// one) on `session` one after the other, then its check, and measure
/// it.
pub(super) async fn run_turns(
    ctx: &Context,
    mut session: AgentSession,
    run: &RunSpec,
    copy: CompareCopy,
) -> Ran {
    let parts = begin(ctx, &mut session, run);
    let prompted = prompt_all(&parts, &mut session, &copy).await;
    let checked = if prompted.turns.is_empty() {
        Checked::default()
    } else {
        check(&parts, &session, &copy, prompted.last.as_ref()).await
    };
    let agent_id = session.agent_id();
    let patch = patch_of(&copy, agent_id).await;
    let tokens = ctx.hub.tokens_by_model(agent_id);
    let cost_usd = tokens.iter().map(|row| row.cost_usd).sum();
    let readings = ctx.hub.plan_readings(agent_id);
    end(&parts, session, prompted.error.is_some()).await;
    Ran {
        run: CompareRun {
            name: run.name.clone(),
            arm: run.arm,
            agent_id,
            prompt_tx: prompted.prompt_tx,
            model: prompted.model,
            files_changed: u32::try_from(changes::files_in(&patch)).unwrap_or(u32::MAX),
            copy,
            wall_ms: prompted.wall_ms,
            cost_usd,
            tokens,
            plan_usage: Vec::new(),
            turns: prompted.turns,
            check_passed: checked.passed,
            error: prompted.error,
            task: run.example.map(|task| task.id.clone()),
        },
        order: (run.n, run.arm),
        reply: prompted.reply,
        patch,
        note: None,
        readings,
        checked: checked.outcome,
    }
}

/// Set `session` up for `run`: unattended, routed as its arm is, building
/// the plan when the work is one, and listed as running. Returns what its
/// turns share.
fn begin<'a>(ctx: &'a Context, session: &mut AgentSession, run: &'a RunSpec) -> RunParts<'a> {
    let hub = &ctx.hub;
    let agent_id = session.agent_id();
    // The run is unattended and works in its own copy, so it runs at yolo:
    // nothing asks, and its commands may only write inside the copy.
    let permissions = Permissions::new(PermissionMode::Agent, ApprovalLevel::Yolo);
    hub.set_permissions(agent_id, permissions);
    modes::apply(&session.kernel, permissions, &hub.tool_names());
    let routing = &ctx.routing[run.arm.index()];
    let gate = &ctx.gates[run.arm.index()];
    hub.begin_compare_run(agent_id, routing.clone(), Arc::clone(gate));
    let task = run
        .example
        .map_or(ctx.plan.task.as_str(), |task| task.title.as_str());
    let recorder = run_recorder(ctx, session, run, task);
    let workdir = session.kernel.agent_workspace();
    if let Some(path) = ctx.plan.plan.as_deref().filter(|path| is_plan_path(path)) {
        if let Ok(plan_file) = PlanFile::load(&workdir.join(path)) {
            session.todos = todo_items(&plan_file);
            session.active_plan = Some(path.to_string());
            session.building_plan = true;
        }
    }
    let cancel = ctx.cancel.child_token();
    let ui = AgentUi::new(ctx.commands.clone(), agent_id);
    let spec = &ctx.plan.arms[run.arm.index()];
    let mut row = turn_summary(agent_id, task, &short_label(spec));
    row.label = format!("{} {}: {}", ctx.host.name, run.name, row.label);
    hub.processes.start(row, Some(cancel.clone()));
    hub.start_running(agent_id, Arc::clone(&session.kernel), cancel.clone(), None);
    RunParts {
        ctx,
        run,
        routing,
        gate,
        cancel,
        ui,
        recorder,
    }
}

/// Send the prompts of `parts`' run on `session` one after the other,
/// until one ends in an error or the user stops the run. A Gate run's
/// review sees what each prompt's turn changed in `copy`.
async fn prompt_all(
    parts: &RunParts<'_>,
    session: &mut AgentSession,
    copy: &CompareCopy,
) -> Prompted {
    let (hub, run) = (&parts.ctx.hub, parts.run);
    let agent_id = session.agent_id();
    let prompts = (run.example).map_or(parts.ctx.plan.prompts.as_slice(), |task| {
        task.prompts.as_slice()
    });
    let started = Instant::now();
    let mut prompted = Prompted {
        turns: Vec::with_capacity(prompts.len()),
        ..Prompted::default()
    };
    let mut refused = Vec::new();
    for (i, text) in prompts.iter().enumerate() {
        if parts.cancel.is_cancelled() {
            prompted.error = Some(STOPPED.to_string());
            break;
        }
        let spent_before = spent(hub, agent_id);
        // What the copy holds before the prompt, so a review of a Gate
        // run sees everything the prompt's turn changes.
        let before = match (parts.gate_arm(), i) {
            (false, _) => None,
            (true, 0) => Some(Before::of_copy(copy)),
            (true, _) => Before::of_copy_now(copy).await,
        };
        let prompt = Prompt {
            text,
            i,
            of: prompts.len(),
            before,
            refused: &mut refused,
        };
        let turned = run_prompt(parts, session, prompt).await;
        if i == 0 {
            prompted.prompt_tx = turned.prompt_tx;
            prompted.model.clone_from(&turned.turn.model);
        }
        let turn = spent(hub, agent_id).since(spent_before, turned.turn);
        prompted.turns.push(turn);
        prompted.reply = turned.reply;
        prompted.last = turned.ended.filter(|_| !turned.cut_short);
        if turned.error.is_some() {
            prompted.error = turned.error;
            break;
        }
    }
    prompted.wall_ms = elapsed_ms(started);
    prompted
}

/// Hand `session` back once its run ended, `failed` or not: its process
/// ends, its subtasks are no longer routed as its arm's, and its trace
/// is closed.
async fn end(parts: &RunParts<'_>, session: AgentSession, failed: bool) {
    let hub = &parts.ctx.hub;
    let agent_id = session.agent_id();
    let status = if failed {
        ProcessStatus::Failed
    } else {
        ProcessStatus::Exited
    };
    hub.processes.finish(&turn_id(agent_id), status);
    hub.end_compare_run(agent_id);
    if parts.recorder.is_some() {
        hub.traces.close(agent_id);
    }
    hub.park(session);
    hub.send_agent_list().await;
}

/// What `agent`'s calls and its subtasks' have cost so far, and how many
/// of them had no known price.
fn spent(hub: &AgentHub, agent: AgentId) -> Spent {
    let rows = hub.tokens_by_model(agent);
    Spent {
        usd: rows.iter().map(|row| row.cost_usd).sum(),
        unpriced_calls: rows.iter().map(|row| row.unpriced_calls).sum(),
    }
}

/// What an agent's priced calls have cost so far, and the calls with no
/// known price that leaves out.
#[derive(Clone, Copy)]
struct Spent {
    usd: f64,
    unpriced_calls: u32,
}

impl Spent {
    /// `turn`, costing what was spent from `before` to this.
    fn since(self, before: Self, turn: RunTurn) -> RunTurn {
        RunTurn {
            cost_usd: (self.usd - before.usd).max(0.0),
            unpriced_calls: self.unpriced_calls.saturating_sub(before.unpriced_calls),
            ..turn
        }
    }
}

/// The trace of `run` on `session`, working on `task`, recorded as a
/// conversation's is, so its LOG tab shows it as it runs and its bundle
/// keeps a `log.json`. `None` when tracing is off.
fn run_recorder(
    ctx: &Context,
    session: &AgentSession,
    run: &RunSpec,
    task: &str,
) -> Option<Arc<RunRecorder>> {
    let spec = &ctx.plan.arms[run.arm.index()];
    let mut meta = RunMeta::new(short_label(spec), RunSource::Bench);
    meta.workspace = session.kernel.agent_workspace().display().to_string();
    meta.mode = PermissionMode::Agent.name().to_string();
    meta.title = format!("{} {}: {task}", ctx.host.name, run.name);
    meta.arm = Some(arm_name(run.arm, spec));
    meta.task_id = run.example.map(|task| task.id.clone());
    ctx.hub.traces.start(session.agent_id(), meta)
}
