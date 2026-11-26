//! One turn of a run, and each of its steps, as the LOG tab lays them out.

use std::collections::HashMap;

use aura_agent_trace::run_log::{LogEvent, LogStep, LogToolCall, LogTurn};
use aura_ui::events::{LogField, LogSection, LogStatus, LogStepView, LogTurnView};

use super::super::compare::view::{duration, headline, money, tokens};
use super::event::event_row;
use super::{count, field, pretty, time};

/// `turn` laid out, its spawned runs found by run id in `below`.
pub(super) fn turn_view(turn: &LogTurn, below: &HashMap<&str, usize>) -> LogTurnView {
    let calls: HashMap<&str, &LogToolCall> = turn
        .steps
        .iter()
        .filter_map(|step| match &step.event {
            LogEvent::ModelCall { tool_calls, .. } => Some(tool_calls),
            _ => None,
        })
        .flatten()
        .map(|call| (call.tool_use_id.as_str(), call))
        .collect();
    let steps: Vec<LogStepView> = turn
        .steps
        .iter()
        .map(|s| step_view(s, below, &calls))
        .collect();
    let named: Vec<usize> = steps.iter().flat_map(|s| s.spawned.clone()).collect();
    let spawned = turn
        .subagents
        .iter()
        .filter_map(|run| below.get(run.as_str()).copied())
        .filter(|i| !named.contains(i))
        .collect();
    let detail = [
        count(turn.model_calls, "call"),
        turn.stop_reason
            .clone()
            .unwrap_or_else(|| "running".to_string()),
    ];
    let status = turn_status(turn, &steps);
    LogTurnView {
        turn: turn.turn,
        prompt: turn
            .prompt
            .as_deref()
            .map_or_else(String::new, |p| headline(p, 200)),
        detail: detail.join(" · "),
        tokens: format!(
            "{} tok",
            tokens(turn.usage.input + turn.usage.output + turn.usage.cache_read)
        ),
        cost: turn.cost_usd.map(money).unwrap_or_default(),
        took: turn.duration_ms.map(duration).unwrap_or_default(),
        status,
        steps,
        spawned,
        json: pretty(turn),
    }
}

/// How `turn` went, given how its `steps` did.
fn turn_status(turn: &LogTurn, steps: &[LogStepView]) -> LogStatus {
    if turn.error.is_some() {
        LogStatus::Error
    } else if turn.stop_reason.is_none() {
        LogStatus::Running
    } else if turn.tool_errors > 0 || steps.iter().any(|s| s.status == LogStatus::Warn) {
        LogStatus::Warn
    } else {
        LogStatus::Ok
    }
}

pub(super) fn section(title: &str, body: impl Into<String>, code: bool) -> LogSection {
    LogSection {
        title: title.to_string(),
        body: body.into(),
        code,
    }
}

/// What a step's row shows beside its number and columns.
pub(super) struct Row {
    pub(super) kind: &'static str,
    pub(super) label: String,
    pub(super) summary: String,
    pub(super) status: LogStatus,
}

impl Row {
    pub(super) fn new(
        kind: &'static str,
        label: String,
        summary: String,
        status: LogStatus,
    ) -> Self {
        Self {
            kind,
            label,
            summary,
            status,
        }
    }
}

/// What a step's detail pane lists, filled in as its event is read.
pub(super) struct Parts<'a> {
    pub(super) fields: Vec<LogField>,
    pub(super) sections: Vec<LogSection>,
    pub(super) spawned: Vec<usize>,
    pub(super) tokens: String,
    pub(super) cost: String,
    pub(super) below: &'a HashMap<&'a str, usize>,
}

/// `step` laid out, its tool results labelled by the call they answer in
/// `calls`, by tool use id.
fn step_view(
    step: &LogStep,
    below: &HashMap<&str, usize>,
    calls: &HashMap<&str, &LogToolCall>,
) -> LogStepView {
    let mut parts = Parts {
        fields: step_fields(step),
        sections: Vec::new(),
        spawned: Vec::new(),
        tokens: String::new(),
        cost: String::new(),
        below,
    };
    let row = event_row(&mut parts, calls, &step.event);
    let Parts {
        mut fields,
        sections,
        spawned,
        tokens,
        cost,
        ..
    } = parts;
    push_links(step, &mut fields);
    LogStepView {
        seq: step.seq,
        kind: row.kind.to_string(),
        label: row.label,
        summary: row.summary,
        parent: step.parent.filter(|_| row.kind == "tool"),
        tokens,
        cost,
        took: step.duration_ms.map(duration).unwrap_or_default(),
        status: row.status,
        fields,
        sections,
        spawned,
        json: pretty(step),
    }
}

/// The fields every step has, before its event's.
fn step_fields(step: &LogStep) -> Vec<LogField> {
    let mut fields = vec![
        field("seq", step.seq.to_string()),
        field("time", time(step.at_ms)),
        field("actor", &step.actor),
    ];
    if let Some(i) = step.iteration {
        fields.push(field("iteration", i.to_string()));
    }
    if let Some(ms) = step.duration_ms {
        fields.push(field("took", duration(ms)));
    }
    fields
}

/// The kernel records `step` wrote and the steps that caused it.
fn push_links(step: &LogStep, fields: &mut Vec<LogField>) {
    if !step.record_seqs.is_empty() {
        let seqs: Vec<String> = step.record_seqs.iter().map(u64::to_string).collect();
        fields.push(field("kernel record", seqs.join(", ")));
    }
    if !step.caused_by.is_empty() {
        let seqs: Vec<String> = step.caused_by.iter().map(u64::to_string).collect();
        fields.push(field("caused by step", seqs.join(", ")));
    }
}
