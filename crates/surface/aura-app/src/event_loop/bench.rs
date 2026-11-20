//! The BENCH tab's requests, served by `/compare`'s actions: every
//! benchmark is a comparison, shown as a [`BenchView`].

use super::agent_events::AgentUi;
use super::compare;
use super::LoopState;
use aura_agent_gate::compare::{
    judge_cost_usd, summarize, task_cells, turn_cells, CompareRequest, SessionPick,
};
use aura_core_types::{
    AgentId, BenchSource, CompareArm, ComparePayload, CompareStatus, ModelTokens, RefVerdict,
};
use aura_ui_terminal::events::{
    BenchArm, BenchBaseline, BenchJudge, BenchRequest, BenchRun, BenchSpec, BenchStatus,
    BenchTaskCell, BenchTaskRow, BenchTokens, BenchTurn, BenchVerdict, BenchView, BenchWork,
};
use aura_ui_terminal::UiCommand;

/// A request from the BENCH tab shown for `agent_id`'s chat.
pub(super) async fn handle(state: &mut LoopState<'_>, agent_id: AgentId, request: BenchRequest) {
    state.park_finished_spawns().await;
    let notice = match request {
        BenchRequest::List => {
            let views = list(state);
            let _ = state.commands.send(UiCommand::BenchList(views)).await;
            return;
        }
        BenchRequest::Sessions => {
            let sessions = super::bench_source::sessions(state, agent_id);
            let examples = crate::bench_pack::tasks()
                .iter()
                .map(|task| task.title.clone())
                .collect();
            let _ = state
                .commands
                .send(UiCommand::BenchSessions {
                    agent_id: agent_id.to_hex(),
                    sessions,
                    examples,
                })
                .await;
            return;
        }
        BenchRequest::Prepare(spec) => {
            let estimate = compare::prepare(state, agent_id, request_of(spec)).await;
            let _ = state
                .commands
                .send(UiCommand::BenchEstimate {
                    agent_id: agent_id.to_hex(),
                    estimate,
                })
                .await;
            return;
        }
        BenchRequest::Start => compare::start(state).await,
        BenchRequest::Cancel => Ok(compare::cancel(state)),
        BenchRequest::Pick { id, arm } => match host(&id) {
            Ok(host) => {
                let arm = CompareArm::BOTH[arm.min(1)].as_str();
                compare::pick(state, Some(host), arm).await
            }
            Err(problem) => Err(problem),
        },
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
        return;
    }
    let _ = state.commands.send(UiCommand::BenchNotice(notice)).await;
}

fn host(id: &str) -> Result<AgentId, String> {
    AgentId::from_hex(id).map_err(|_| format!("No benchmark {id}"))
}

/// The comparison the BENCH tab's form set up.
fn request_of(spec: BenchSpec) -> CompareRequest {
    let (prompt, session, examples) = match spec.work {
        BenchWork::Prompt(prompt) => (Some(prompt), None, None),
        BenchWork::LastPrompt => (None, None, None),
        BenchWork::CurrentSession => (None, Some(SessionPick::Current), None),
        BenchWork::Session(started_ms) => (None, Some(SessionPick::Started(started_ms)), None),
        BenchWork::Examples { tasks } => (None, None, Some(tasks)),
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
    }
}

/// Every benchmark with a record, oldest first.
fn list(state: &LoopState<'_>) -> Vec<BenchView> {
    compare::comparisons(state)
        .into_iter()
        .filter_map(|(id, name, payload)| Some(view(id, &name, &payload?)))
        .collect()
}

fn status(status: &CompareStatus) -> BenchStatus {
    match status {
        CompareStatus::Running => BenchStatus::Running,
        CompareStatus::Done => BenchStatus::Done,
        CompareStatus::Cancelled => BenchStatus::Cancelled,
        CompareStatus::Picked(arm) => BenchStatus::Picked(arm.index()),
        CompareStatus::Applied(run) => BenchStatus::Applied(run.clone()),
        CompareStatus::Dismissed => BenchStatus::Dismissed,
    }
}

fn tokens(rows: &[ModelTokens]) -> Vec<BenchTokens> {
    rows.iter()
        .map(|row| BenchTokens {
            model: row.model.clone(),
            calls: row.calls,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cost_usd: row.cost_usd,
        })
        .collect()
}

fn arm(payload: &ComparePayload, arm: CompareArm) -> BenchArm {
    let summary = summarize(payload, arm);
    BenchArm {
        label: payload.spec(arm).label().to_string(),
        runs: summary.runs,
        finished: summary.finished,
        total_cost_usd: summary.total_cost_usd,
        median_cost_usd: summary.median_cost_usd,
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
                passed: cell.passed,
                failed: cell.failed,
                errors: cell.errors,
            })
            .collect(),
    }
}

fn verdict(verdict: Option<RefVerdict>) -> Option<String> {
    verdict.map(|v| v.label().to_string())
}

/// The comparison held by `host` (named `name`) as the BENCH tab shows it.
pub(super) fn view(host: AgentId, name: &str, payload: &ComparePayload) -> BenchView {
    let (title, source) = match &payload.source {
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
    };
    let cells = CompareArm::BOTH.map(|arm| task_cells(payload, arm));
    let tasks = payload
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
                    })
                    .unwrap_or_default()
            }),
        })
        .collect();
    let judge = payload.judgments.first().map(|first| BenchJudge {
        model: first.model.clone(),
        cost_usd: judge_cost_usd(payload),
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
    });
    BenchView {
        id: host.to_hex(),
        name: name.to_string(),
        title,
        started_ms: payload.started_ms,
        source,
        status: status(&payload.status),
        repeats: payload.repeats,
        tasks,
        check: payload.check.clone(),
        budget_usd: payload.budget_usd,
        prompts: payload.prompts.clone(),
        arms: [arm(payload, CompareArm::A), arm(payload, CompareArm::B)],
        runs: payload
            .runs
            .iter()
            .map(|run| BenchRun {
                name: run.name.clone(),
                arm: run.arm.index(),
                model: run.model.clone(),
                wall_ms: run.wall_ms,
                cost_usd: run.cost_usd,
                check_passed: run.check_passed,
                files_changed: run.files_changed,
                error: run.error.clone(),
                task: run.task.clone(),
            })
            .collect(),
        judge,
        baseline: payload.source.baseline().map(|b| BenchBaseline {
            calls: b.calls,
            cost_usd: b.cost_usd,
            priced: b.priced,
            models: b.models.clone(),
        }),
        has_reference: payload.source.reference().is_some(),
        notes: payload.notes.clone(),
    }
}
