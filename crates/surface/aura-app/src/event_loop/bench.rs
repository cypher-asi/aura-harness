//! The BENCH tab's requests, served by `/compare`'s actions: every
//! benchmark is a comparison, shown as a [`BenchView`].

#![deny(clippy::too_many_lines)]

use super::agent_events::AgentUi;
use super::compare;
use super::subagents::lock;
use super::LoopState;
use aura_core_types::{
    AgentId, Assessment, BenchSource, CompareArm, ComparePayload, CompareRun, CompareStatus,
    ModelTokens, RefVerdict, RunTurn,
};
use aura_gate::compare::{
    judge_cost_usd, judge_unpriced_calls, summarize, task_cells, turn_cells, CompareRequest,
    SessionPick,
};
use aura_ui::events::{
    run_order, BenchArm, BenchBaseline, BenchExample, BenchJudge, BenchRequest, BenchRun,
    BenchSpec, BenchStatus, BenchTaskCell, BenchTaskRow, BenchTaskSetup, BenchTokens, BenchTurn,
    BenchVerdict, BenchView, BenchWork,
};
use aura_ui::UiCommand;

/// A request from the BENCH tab for `agent_id`, the agent a new benchmark
/// starts from or whose chat shows a run's diff.
pub(super) async fn handle(state: &mut LoopState<'_>, agent_id: AgentId, request: BenchRequest) {
    state.park_finished_spawns().await;
    if let Some(reply) = reply(state, agent_id, request).await {
        let _ = state.commands.send(reply).await;
    }
}

/// What the BENCH tab is sent after `request`: what it asked for, or the
/// notice an action leaves; `None` when the action has nothing to say.
async fn reply(
    state: &mut LoopState<'_>,
    agent_id: AgentId,
    request: BenchRequest,
) -> Option<UiCommand> {
    let notice = match request {
        BenchRequest::List => {
            return Some(UiCommand::BenchList {
                benches: list(state),
            })
        }
        BenchRequest::Sessions => return Some(bench_sessions(state, agent_id)),
        BenchRequest::Prepare(spec) => {
            let estimate = compare::prepare(state, agent_id, request_of(spec)).await;
            return Some(UiCommand::BenchEstimate {
                agent_id: agent_id.to_hex(),
                estimate,
            });
        }
        BenchRequest::Start => compare::start(state).await,
        BenchRequest::Cancel => Ok(compare::cancel(state)),
        BenchRequest::Apply { id, run } => match host(&id) {
            Ok(host) => compare::apply(state, Some(host), &run).await,
            Err(problem) => Err(problem),
        },
        BenchRequest::Diff { id, run } => match host(&id) {
            Ok(host) => {
                let ui = AgentUi::new(state.commands.clone(), agent_id);
                compare::diff(state, &ui, Some(host), run.as_deref()).await
            }
            Err(problem) => Err(problem),
        },
        BenchRequest::Dismiss { id } => match host(&id) {
            Ok(host) => compare::dismiss(state, Some(host)).await,
            Err(problem) => Err(problem),
        },
    };
    if notice.as_ref().is_ok_and(String::is_empty) {
        return None;
    }
    Some(UiCommand::BenchNotice(notice))
}

/// The sessions of `agent_id` the BENCH tab can replay, and the example
/// tasks it can run.
fn bench_sessions(state: &LoopState<'_>, agent_id: AgentId) -> UiCommand {
    let sessions = super::bench_source::sessions(state, agent_id);
    let examples = crate::bench_pack::tasks()
        .iter()
        .map(|task| BenchExample {
            suite: task.suite().to_string(),
            title: task.title.clone(),
        })
        .collect();
    UiCommand::BenchSessions {
        agent_id: agent_id.to_hex(),
        sessions,
        examples,
    }
}

fn host(id: &str) -> Result<AgentId, String> {
    AgentId::from_hex(id).map_err(|_| format!("No benchmark {id}"))
}

/// The comparison the BENCH tab's form set up.
fn request_of(spec: BenchSpec) -> CompareRequest {
    let (prompt, session, examples, suite) = match spec.work {
        BenchWork::Prompt(prompt) => (Some(prompt), None, None, None),
        BenchWork::LastPrompt => (None, None, None, None),
        BenchWork::CurrentSession => (None, Some(SessionPick::Current), None, None),
        BenchWork::Session(started_ms) => {
            (None, Some(SessionPick::Started(started_ms)), None, None)
        }
        BenchWork::Examples { suite, tasks } => (None, None, Some(tasks), suite),
    };
    CompareRequest {
        prompt,
        plan: None,
        session,
        examples,
        a: Some(spec.a),
        against: Some(spec.b),
        repeats: spec.repeats.max(1),
        check: spec.check,
        budget_usd: spec.budget_usd,
        learn: false,
        suite,
        classifier: spec.classifier,
    }
}

/// Every benchmark with a record, of every tree, oldest first.
fn list(state: &LoopState<'_>) -> Vec<BenchView> {
    let all = compare::comparisons(state);
    let registry = lock(&state.hub.registry);
    all.into_iter()
        .filter_map(|(id, name, payload)| Some(view(id, registry.root(id), &name, &payload?)))
        .collect()
}

fn status(status: &CompareStatus) -> BenchStatus {
    match status {
        CompareStatus::Running => BenchStatus::Running,
        CompareStatus::Done => BenchStatus::Done,
        CompareStatus::Cancelled => BenchStatus::Cancelled,
        CompareStatus::Applied(run) => BenchStatus::Applied(run.clone()),
        CompareStatus::Dismissed => BenchStatus::Dismissed,
    }
}

pub(super) fn tokens(rows: &[ModelTokens]) -> Vec<BenchTokens> {
    rows.iter()
        .map(|row| BenchTokens {
            model: row.model.clone(),
            purpose: row.purpose.clone(),
            calls: row.calls,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cost_usd: row.cost_usd,
            unpriced_calls: row.unpriced_calls,
        })
        .collect()
}

fn arm(payload: &ComparePayload, arm: CompareArm) -> BenchArm {
    let summary = summarize(payload, arm);
    let runs_where = |moved: fn(&RunTurn) -> bool| {
        payload
            .runs
            .iter()
            .filter(|run| run.arm == arm && run.turns.iter().any(moved))
            .count()
    };
    BenchArm {
        label: payload.spec(arm).label().to_string(),
        runs: summary.runs,
        finished: summary.finished,
        total_cost_usd: summary.total_cost_usd,
        median_cost_usd: summary.median_cost_usd,
        unpriced_calls: summary.unpriced_calls,
        finished_unpriced_calls: summary.finished_unpriced_calls,
        median_wall_ms: summary.median_wall_ms,
        checks_passed: summary.checks_passed,
        checks_run: summary.checks_run,
        judge_wins: summary.judge_wins,
        judge_losses: summary.judge_losses,
        judge_ties: summary.judge_ties,
        ref_better: summary.ref_better,
        ref_as_good: summary.ref_as_good,
        ref_worse: summary.ref_worse,
        tokens: tokens(&summary.tokens),
        turns: turn_cells(payload, arm)
            .into_iter()
            .map(|cell| BenchTurn {
                runs: cell.runs,
                mean_cost_usd: cell.mean_cost_usd,
                unpriced_calls: cell.unpriced_calls,
                passed: cell.passed,
                failed: cell.failed,
                errors: cell.errors,
            })
            .collect(),
        escalated: runs_where(|t| t.escalations > 0),
        nudged: runs_where(|t| t.nudges > 0),
    }
}

/// The model `run`'s first turn was sent to, the one a failed attempt
/// last moved its work to (the same model, when the move only raised
/// its effort), and that effort when the move raised it.
fn models(run: &CompareRun) -> (String, Option<String>, Option<String>) {
    let first = run.turns.first().map_or_else(
        || run.model.clone(),
        |turn| {
            turn.first_model
                .clone()
                .unwrap_or_else(|| turn.model.clone())
        },
    );
    let Some(moved) = run.turns.iter().rev().find(|turn| turn.escalations > 0) else {
        return (first, None, None);
    };
    let effort = moved
        .first_effort
        .is_some()
        .then(|| moved.effort.clone())
        .flatten();
    let model = (moved.model != first || effort.is_some()).then(|| moved.model.clone());
    (first, model, effort)
}

fn verdict(verdict: Option<RefVerdict>) -> Option<String> {
    verdict.map(|v| v.label().to_string())
}

/// The comparison held by `host` (named `name`) in the tree under `root`
/// as the BENCH tab shows it.
pub(super) fn view(
    host: AgentId,
    root: AgentId,
    name: &str,
    payload: &ComparePayload,
) -> BenchView {
    let (title, source) = title_and_source(payload);
    let mut runs: Vec<BenchRun> = payload.runs.iter().map(run).collect();
    runs.sort_by_key(|run| run_order(&run.name, run.arm));
    BenchView {
        id: host.to_hex(),
        root: root.to_hex(),
        name: name.to_string(),
        title,
        started_ms: payload.started_ms,
        source,
        status: status(&payload.status),
        repeats: payload.repeats,
        tasks: tasks(payload),
        check: payload.check.clone(),
        budget_usd: payload.budget_usd,
        classifier: payload.classifier.clone(),
        prompts: payload.prompts.clone(),
        arms: [arm(payload, CompareArm::A), arm(payload, CompareArm::B)],
        runs,
        judge: judge(payload),
        classify: tokens(&payload.classify_tokens),
        baseline: payload.source.baseline().map(|b| BenchBaseline {
            calls: b.calls,
            cost_usd: b.cost_usd,
            priced: b.priced,
            unpriced_calls: b.unpriced_calls,
            models: b.models.clone(),
        }),
        has_reference: payload.source.reference().is_some(),
        notes: payload.notes.clone(),
        live: Vec::new(),
    }
}

/// The comparison's title, and where its work came from.
fn title_and_source(payload: &ComparePayload) -> (String, String) {
    match &payload.source {
        BenchSource::Session { title, .. } => (title.clone(), format!("session \"{title}\"")),
        BenchSource::Prompt => {
            let title = payload.task.lines().next().unwrap_or_default().to_string();
            let source = payload
                .plan
                .as_ref()
                .map_or_else(|| "prompt".to_string(), |plan| format!("plan {plan}"));
            (title, source)
        }
        BenchSource::Examples { tasks } => (
            payload.task.clone(),
            format!(
                "{} example task{}",
                tasks.len(),
                if tasks.len() == 1 { "" } else { "s" }
            ),
        ),
    }
}

/// The comparison's example tasks, each with how either arm did on it.
fn tasks(payload: &ComparePayload) -> Vec<BenchTaskRow> {
    let cells = CompareArm::BOTH.map(|arm| task_cells(payload, arm));
    payload
        .source
        .examples()
        .iter()
        .enumerate()
        .map(|(i, task)| BenchTaskRow {
            id: task.id.clone(),
            title: task.title.clone(),
            arms: CompareArm::BOTH.map(|arm| {
                cells[arm.index()]
                    .get(i)
                    .map(|cell| BenchTaskCell {
                        runs: cell.runs,
                        passed: cell.passed,
                        checked: cell.checked,
                        mean_cost_usd: cell.mean_cost_usd,
                        unpriced_calls: cell.unpriced_calls,
                    })
                    .unwrap_or_default()
            }),
            setup: BenchTaskSetup::Ready,
        })
        .collect()
}

/// The judge's verdicts on the comparison's pairs; `None` before it gave
/// any.
fn judge(payload: &ComparePayload) -> Option<BenchJudge> {
    let first = payload.judgments.first()?;
    Some(BenchJudge {
        model: first.model.clone(),
        cost_usd: judge_cost_usd(payload),
        unpriced_calls: judge_unpriced_calls(payload),
        verdicts: payload
            .judgments
            .iter()
            .map(|j| BenchVerdict {
                a_run: j.a_run.clone(),
                b_run: j.b_run.clone(),
                parsed: j.parsed,
                winner: j.winner.map(CompareArm::index),
                a_vs_reference: verdict(j.a_vs_reference),
                b_vs_reference: verdict(j.b_vs_reference),
                reasoning: j.reasoning.clone(),
            })
            .collect(),
    })
}

/// `run` as the BENCH tab lists it.
fn run(run: &CompareRun) -> BenchRun {
    let (model, escalated_to, escalated_effort) = models(run);
    let first = run.turns.first();
    BenchRun {
        name: run.name.clone(),
        arm: run.arm.index(),
        model,
        escalated_to,
        escalated_effort,
        category: first
            .and_then(|turn| turn.category)
            .map(|category| category.as_str().to_string()),
        difficulty: first
            .and_then(|turn| turn.difficulty)
            .map(|difficulty| difficulty.as_str().to_string()),
        drivers: first
            .and_then(|turn| turn.assessment.as_ref())
            .map(Assessment::summary),
        led: run.turns.iter().any(|turn| turn.led == Some(true)),
        subtask_cost_usd: run.turns.iter().map(|turn| turn.subtask_cost_usd).sum(),
        wall_ms: run.wall_ms,
        cost_usd: run.cost_usd,
        unpriced_calls: run.unpriced_calls(),
        check_passed: run.check_passed,
        files_changed: run.files_changed,
        error: run.error.clone(),
        task: run.task.clone(),
        agent_id: run.agent_id.to_hex(),
    }
}
