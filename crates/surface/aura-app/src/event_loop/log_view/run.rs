//! A run of the LOG tab: its row, its fields and the files it was written to.

use std::collections::HashMap;

use aura_agent_trace::RunLog;
use aura_ui::events::{LogField, LogRunView, LogStatus, LogTurnView};

use super::super::compare::view::{duration, money, tokens};
use super::step::turn_view;
use super::{count, field, pretty, repeats, short_model, time};

/// Add `log` and the runs below it to `runs`, each after its spawner.
/// Returns the index of `log`'s run.
pub(super) fn push_run(runs: &mut Vec<LogRunView>, log: &RunLog, depth: usize) -> usize {
    let index = runs.len();
    runs.push(LogRunView::default());
    let mut below: HashMap<&str, usize> = HashMap::new();
    for child in &log.subagents {
        below.insert(&child.run.run_id, push_run(runs, child, depth + 1));
    }
    let mut turns: Vec<LogTurnView> = log
        .turns
        .iter()
        .map(|turn| turn_view(turn, &below))
        .collect();
    let others = others_of(log, &below);
    let run = &log.run;
    let title = if run.title.is_empty() {
        format!("run {}", run.run_id)
    } else {
        run.title.clone()
    };
    for turn in &mut turns {
        if repeats(&title, &turn.prompt) {
            turn.prompt.clear();
        }
    }
    let status = run_status(log, runs, &others);
    runs[index] = LogRunView {
        run_id: run.run_id.clone(),
        title,
        detail: run_detail(log, &others),
        status,
        depth,
        fields: run_fields(log),
        files: file_fields(log),
        problems: log.health.problems.clone(),
        turns,
        runs: others,
        json: pretty(log),
    };
    index
}

/// The indexes in `below` of `log`'s runs that none of its turns spawned.
fn others_of(log: &RunLog, below: &HashMap<&str, usize>) -> Vec<usize> {
    let in_turns: Vec<&str> = log
        .turns
        .iter()
        .flat_map(|t| t.subagents.iter().map(String::as_str))
        .collect();
    log.subagents
        .iter()
        .filter(|child| !in_turns.contains(&child.run.run_id.as_str()))
        .filter_map(|child| below.get(child.run.run_id.as_str()).copied())
        .collect()
}

/// How `log`'s run did, as its runs in `runs` at `others` did when it has
/// no turns of its own.
fn run_status(log: &RunLog, runs: &[LogRunView], others: &[usize]) -> LogStatus {
    let run = &log.run;
    let grouped = |status: LogStatus| others.iter().any(|&i| runs[i].status == status);
    if log.turns.is_empty() && !others.is_empty() {
        // An agent with no run of its own is as its runs are.
        [LogStatus::Running, LogStatus::Error, LogStatus::Warn]
            .into_iter()
            .find(|&status| grouped(status))
            .unwrap_or(LogStatus::Ok)
    } else if log.turns.iter().any(|t| t.error.is_some()) {
        LogStatus::Error
    } else if run.ended_at_ms.is_none() || log.turns.last().is_some_and(|t| t.stop_reason.is_none())
    {
        LogStatus::Running
    } else if log.health.problems.is_empty() {
        LogStatus::Ok
    } else {
        LogStatus::Warn
    }
}

/// The run's row detail: model, how many turns or runs, cost and time.
fn run_detail(log: &RunLog, others: &[usize]) -> String {
    let run = &log.run;
    let mut detail: Vec<String> = Some(short_model(&run.model))
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .into_iter()
        .collect();
    if log.turns.is_empty() && !others.is_empty() {
        detail.push(count(others.len(), "run"));
    } else {
        detail.push(count(log.turns.len(), "turn"));
    }
    if let Some(cost) = log.totals.cost_usd_with_subagents {
        detail.push(money(cost));
    }
    if let Some(ms) = run.duration_ms {
        detail.push(duration(ms));
    }
    if log.totals.subagents > 0 {
        detail.push(count(log.totals.subagents, "subagent"));
    }
    detail.join(" · ")
}

fn run_fields(log: &RunLog) -> Vec<LogField> {
    let run = &log.run;
    if run.source == "agent" {
        return agent_fields(log);
    }
    let mut fields = run_identity(log);
    push_totals(log, &mut fields);
    push_work(log, &mut fields);
    fields.push(field("trace level", &run.trace_level));
    fields.push(field("harness version", &run.harness_version));
    fields
}

/// The fields of an agent's grouping of its runs.
fn agent_fields(log: &RunLog) -> Vec<LogField> {
    let run = &log.run;
    let totals = &log.totals;
    let mut fields = vec![field("agent", run.agent_id.as_deref().unwrap_or_default())];
    fields.push(field("runs", totals.subagents.to_string()));
    if let Some(cost) = totals.cost_usd_with_subagents {
        fields.push(field("cost", money(cost)));
    }
    fields
}

/// Which run it is, where it ran, when, and what spawned it.
fn run_identity(log: &RunLog) -> Vec<LogField> {
    let run = &log.run;
    let mut fields = vec![field("run", &run.run_id)];
    if let Some(agent) = &run.agent_id {
        fields.push(field("agent", agent));
    }
    fields.push(field("model", &run.model));
    let source = if run.mode.is_empty() {
        run.source.clone()
    } else {
        format!("{} · {}", run.source, run.mode)
    };
    fields.push(field("source", source));
    if !run.workspace.is_empty() {
        fields.push(field("workspace", &run.workspace));
    }
    fields.push(field("started", time(run.started_at_ms)));
    fields.push(field(
        "ended",
        run.ended_at_ms.map_or_else(|| "running".to_string(), time),
    ));
    if let Some(ms) = run.duration_ms {
        fields.push(field("wall time", duration(ms)));
    }
    if let Some(spawn) = &run.spawned_by {
        let mut by = format!("run {}", spawn.run_id);
        if let Some(turn) = spawn.turn {
            by.push_str(&format!(", turn {turn}"));
        }
        if let Some(step) = spawn.step {
            by.push_str(&format!(", step {step}"));
        }
        fields.push(field("spawned by", by));
    }
    fields
}

/// The run's calls, tokens and cost.
fn push_totals(log: &RunLog, fields: &mut Vec<LogField>) {
    let totals = &log.totals;
    fields.push(field(
        "calls",
        format!(
            "{} model, {} tool ({} failed), {} harness decisions",
            totals.model_calls, totals.tool_calls, totals.tool_errors, totals.decisions
        ),
    ));
    fields.push(field(
        "tokens",
        format!(
            "{} in, {} out, {} cache read, {} cache write",
            tokens(totals.usage.input),
            tokens(totals.usage.output),
            tokens(totals.usage.cache_read),
            tokens(totals.usage.cache_write)
        ),
    ));
    if let Some(cost) = totals.cost_usd {
        fields.push(field("cost", money(cost)));
    }
    if totals.subagents > 0 {
        fields.push(field("subagents", totals.subagents.to_string()));
        if let Some(cost) = totals.cost_usd_with_subagents {
            fields.push(field("cost with subagents", money(cost)));
        }
    }
}

/// What the run did to the workspace, and what the harness did to it.
fn push_work(log: &RunLog, fields: &mut Vec<LogField>) {
    let card = &log.scorecard;
    fields.push(field(
        "work",
        format!(
            "{} reads ({} redundant), {} writes, {} failed edits, {} files changed",
            card.reads, card.redundant_reads, card.writes, card.failed_edits, card.files_changed
        ),
    ));
    if card.blocked_calls + card.retries + card.cache_hits > 0 {
        fields.push(field(
            "harness",
            format!(
                "{} blocked, {} retries, {} cache hits",
                card.blocked_calls, card.retries, card.cache_hits
            ),
        ));
    }
    if let Some(ms) = card.ms_to_first_edit {
        fields.push(field("first edit after", duration(ms)));
    }
    if let Some(outcome) = card.outcome {
        fields.push(field("outcome", format!("{outcome:+.2}")));
    }
}

fn file_fields(log: &RunLog) -> Vec<LogField> {
    let files = &log.files;
    let mut fields = vec![
        field("log", &files.log),
        field("folder", &files.bundle),
        field("steps", &files.steps),
        field("record", &files.record),
        field("summary", &files.summary),
    ];
    if let Some(blobs) = &files.blobs {
        fields.push(field("blobs", blobs));
    }
    fields.retain(|(_, path)| !path.is_empty());
    fields
}
