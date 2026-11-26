//! Whether a run is stuck, and where.
//!
//! The kernel record log stamps every tool call when it is proposed,
//! when the policy decides it and when it runs. [`tool_call_timings`]
//! pairs those rows per call, so a call that waited on an approval or
//! never finished shows up even when the run itself never ended.
//! [`check`] turns that, plus the run's steps and metadata, into a
//! [`RunHealth`] report for `aura runs check`.

use std::collections::{BTreeMap, HashMap};

use aura_core_types::trace::StepAction;
use serde::{Deserialize, Serialize};

use crate::records::RecordRow;
use crate::RunTrace;

/// Policy decisions this slow count as approval prompts in record logs
/// written before decisions named their approver; the kernel decides
/// everything else in milliseconds.
pub const APPROVAL_GUESS_MS: u64 = 1_000;

/// Default wait past which a tool call counts as slow.
pub const DEFAULT_SLOW_MS: u64 = 30_000;

/// Quiet time after which a run that never ended counts as abandoned.
pub const STALE_MS: u64 = 10 * 60 * 1_000;

/// How the agent loop starts the result of a call cut off by a cancel.
const CANCELLED: &str = "[CANCELLED]";

/// One tool call as the kernel record log saw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallTiming {
    /// Tool-use id.
    pub tool_use_id: String,
    /// Tool name.
    pub tool: String,
    /// When the model's call reached the kernel.
    pub proposed_at_ms: u64,
    /// When the policy decided it.
    pub decided_at_ms: Option<u64>,
    /// When it ran.
    pub executed_at_ms: Option<u64>,
    /// Gate verdict, when the record names it.
    pub verdict: Option<String>,
    /// Who answered the approval prompt, when the record names one.
    pub approver: Option<String>,
}

impl ToolCallTiming {
    /// Time from proposal to decision.
    #[must_use]
    pub fn decision_ms(&self) -> Option<u64> {
        self.decided_at_ms
            .map(|at| at.saturating_sub(self.proposed_at_ms))
    }

    /// Whether the call waited for someone to approve it.
    #[must_use]
    pub fn asked_for_approval(&self) -> bool {
        self.approver.is_some() || self.decision_ms().is_some_and(|ms| ms >= APPROVAL_GUESS_MS)
    }

    /// Whether the policy refused the call.
    #[must_use]
    pub fn denied(&self) -> bool {
        self.verdict.as_deref() == Some("denied")
    }

    /// Whether the call is still waiting: undecided, or allowed but not
    /// yet run.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.executed_at_ms.is_none() && !self.denied()
    }

    /// What an open call waits for.
    #[must_use]
    pub const fn stage(&self) -> &'static str {
        if self.decided_at_ms.is_none() {
            "awaiting a decision"
        } else {
            "awaiting execution"
        }
    }
}

/// Pair each tool call's proposal, decision and execution rows, in
/// proposal order.
#[must_use]
pub fn tool_call_timings(rows: &[RecordRow]) -> Vec<ToolCallTiming> {
    let mut calls: BTreeMap<u64, ToolCallTiming> = BTreeMap::new();
    let mut proposal_seq: BTreeMap<&str, u64> = BTreeMap::new();
    for row in rows {
        let Some(id) = row.tool_use_id.as_deref() else {
            continue;
        };
        match row.tx_type.as_str() {
            "tool_proposal" => {
                proposal_seq.entry(id).or_insert(row.seq);
                calls.entry(row.seq).or_insert_with(|| ToolCallTiming {
                    tool_use_id: id.to_string(),
                    tool: row.tool.clone().unwrap_or_default(),
                    proposed_at_ms: row.at_ms,
                    decided_at_ms: None,
                    executed_at_ms: None,
                    verdict: None,
                    approver: None,
                });
            }
            "policy_decision" => {
                if let Some(call) = proposal_seq.get(id).and_then(|seq| calls.get_mut(seq)) {
                    call.decided_at_ms.get_or_insert(row.at_ms);
                    call.verdict.clone_from(&row.verdict);
                    call.approver.clone_from(&row.approver);
                }
            }
            "tool_execution" => {
                if let Some(call) = proposal_seq.get(id).and_then(|seq| calls.get_mut(seq)) {
                    call.executed_at_ms.get_or_insert(row.at_ms);
                }
            }
            _ => {}
        }
    }
    calls.into_values().collect()
}

/// A tool call that waited on an approval prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalWait {
    /// Tool-use id.
    pub tool_use_id: String,
    /// Tool name.
    pub tool: String,
    /// How long it waited.
    pub wait_ms: u64,
    /// The answer, when recorded.
    pub verdict: Option<String>,
}

/// A tool call that has not finished.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenCall {
    /// Tool-use id.
    pub tool_use_id: String,
    /// Tool name.
    pub tool: String,
    /// What it waits for.
    pub stage: String,
    /// Time since it was proposed, up to the run's end or now.
    pub waiting_ms: u64,
    /// Whether the turn was cancelled while it waited.
    #[serde(default)]
    pub cancelled: bool,
}

/// What `aura runs check` reports.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunHealth {
    /// Run id.
    pub run_id: String,
    /// Whether the run stamped an end time.
    pub ended: bool,
    /// Time since the last step or record row, for a run that has not
    /// ended.
    pub idle_ms: Option<u64>,
    /// Tool calls that have not finished.
    pub open_calls: Vec<OpenCall>,
    /// Approval waits longer than the slow threshold.
    pub slow_approvals: Vec<ApprovalWait>,
    /// Tool calls that ran but have no result step in `steps.jsonl`.
    pub unrecorded_results: Vec<String>,
    /// Tool calls that ran but whose result told the model they were
    /// cancelled, so the model does not know their effects landed.
    #[serde(default)]
    pub ran_but_reported_cancelled: Vec<String>,
    /// One line per problem found; empty when the run looks healthy.
    pub problems: Vec<String>,
}

impl RunHealth {
    /// Whether nothing looks wrong.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Check `trace` at `now_ms`. Waits longer than `slow_ms` count as
/// problems.
#[must_use]
pub fn check(trace: &RunTrace, now_ms: u64, slow_ms: u64) -> RunHealth {
    let timings = tool_call_timings(&trace.records);
    let ended_at = trace.meta.ended_at_ms;
    let until = ended_at.unwrap_or(now_ms);
    let mut health = RunHealth {
        run_id: trace.meta.run_id.clone(),
        ended: ended_at.is_some(),
        ..RunHealth::default()
    };
    let results = result_cancellations(trace);
    for call in &timings {
        record_call(&mut health, call, &results, until, slow_ms);
    }
    if !health.ended {
        health.idle_ms = Some(now_ms.saturating_sub(last_activity_ms(trace)));
    }
    health.problems = problems(&health);
    health
}

/// Whether each recorded tool result reports a cancellation, by call id.
fn result_cancellations(trace: &RunTrace) -> HashMap<&str, bool> {
    trace
        .steps
        .iter()
        .filter_map(|s| match &s.action {
            StepAction::ToolResult {
                tool_use_id,
                excerpt,
                ..
            } => Some((tool_use_id.as_str(), excerpt.starts_with(CANCELLED))),
            _ => None,
        })
        .collect()
}

fn record_call(
    health: &mut RunHealth,
    call: &ToolCallTiming,
    results: &HashMap<&str, bool>,
    until: u64,
    slow_ms: u64,
) {
    if call.asked_for_approval() {
        let wait_ms = call.decision_ms().unwrap_or_default();
        if wait_ms >= slow_ms {
            health.slow_approvals.push(ApprovalWait {
                tool_use_id: call.tool_use_id.clone(),
                tool: call.tool.clone(),
                wait_ms,
                verdict: call.verdict.clone(),
            });
        }
    }
    if call.is_open() {
        let waiting_ms = until.saturating_sub(call.proposed_at_ms);
        if health.ended || waiting_ms >= slow_ms {
            health.open_calls.push(OpenCall {
                tool_use_id: call.tool_use_id.clone(),
                tool: call.tool.clone(),
                stage: call.stage().to_string(),
                waiting_ms,
                cancelled: results.get(call.tool_use_id.as_str()) == Some(&true),
            });
        }
    }
    if call.executed_at_ms.is_some() {
        match results.get(call.tool_use_id.as_str()) {
            None => health.unrecorded_results.push(call.tool_use_id.clone()),
            Some(true) => health
                .ran_but_reported_cancelled
                .push(call.tool_use_id.clone()),
            Some(false) => {}
        }
    }
}

fn last_activity_ms(trace: &RunTrace) -> u64 {
    trace
        .steps
        .iter()
        .map(|s| s.at_ms)
        .chain(trace.records.iter().map(|r| r.at_ms))
        .max()
        .unwrap_or(trace.meta.started_at_ms)
}

fn problems(health: &RunHealth) -> Vec<String> {
    let mut out = Vec::new();
    for call in &health.open_calls {
        let state = if call.cancelled {
            "was cancelled"
        } else if health.ended {
            "never finished"
        } else {
            "has been waiting"
        };
        out.push(format!(
            "{} {} {state} ({}, {})",
            call.tool,
            short_id(&call.tool_use_id),
            call.stage,
            duration(call.waiting_ms)
        ));
    }
    for wait in &health.slow_approvals {
        out.push(format!(
            "{} {} waited {} for approval ({})",
            wait.tool,
            short_id(&wait.tool_use_id),
            duration(wait.wait_ms),
            wait.verdict.as_deref().unwrap_or("answer not recorded")
        ));
    }
    for id in &health.ran_but_reported_cancelled {
        out.push(format!(
            "tool call {} ran, but the model was told it was cancelled",
            short_id(id)
        ));
    }
    if !health.unrecorded_results.is_empty() {
        out.push(format!(
            "{} tool call(s) ran but have no result step",
            health.unrecorded_results.len()
        ));
    }
    if let Some(idle) = health.idle_ms.filter(|ms| *ms >= STALE_MS) {
        out.push(format!(
            "run never ended; nothing recorded for {}",
            duration(idle)
        ));
    }
    out
}

/// Last six characters of a tool-use id, enough to tell the calls of one
/// batch apart.
fn short_id(id: &str) -> &str {
    let start = id.len().saturating_sub(6);
    id.get(start..).unwrap_or(id)
}

/// `4m 04s`, `12s`.
#[must_use]
pub fn duration(ms: u64) -> String {
    let secs = ms / 1_000;
    if secs >= 60 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::{RunMeta, RunSource};

    fn row(seq: u64, tx_type: &str, at_ms: u64, id: &str) -> RecordRow {
        RecordRow {
            seq,
            tx_type: tx_type.into(),
            at_ms,
            tool_use_id: Some(id.into()),
            tool: Some("edit_file".into()),
            verdict: None,
            approver: None,
            rule: None,
            reason: None,
            call: None,
            route: None,
        }
    }

    /// The stalled batch from a real run: three edits proposed together,
    /// only the third ever decided, 244 s later.
    fn stalled_batch() -> Vec<RecordRow> {
        vec![
            row(1, "tool_proposal", 0, "toolu_a"),
            row(2, "policy_decision", 3, "toolu_a"),
            row(3, "tool_execution", 10, "toolu_a"),
            row(4, "tool_proposal", 1_000, "toolu_x"),
            row(5, "tool_proposal", 1_007, "toolu_y"),
            row(6, "tool_proposal", 1_011, "toolu_z"),
            row(7, "policy_decision", 245_679, "toolu_z"),
            row(8, "tool_execution", 245_687, "toolu_z"),
        ]
    }

    #[test]
    fn timings_pair_rows_per_call() {
        let timings = tool_call_timings(&stalled_batch());
        assert_eq!(timings.len(), 4);
        assert!(!timings[0].asked_for_approval());
        assert!(!timings[0].is_open());
        assert!(timings[1].is_open());
        assert_eq!(timings[1].stage(), "awaiting a decision");
        assert!(timings[3].asked_for_approval());
        assert_eq!(timings[3].decision_ms(), Some(244_668));
    }

    #[test]
    fn a_named_approver_counts_even_when_quick() {
        let mut rows = stalled_batch();
        rows[1].approver = Some("user".into());
        rows[1].verdict = Some("approved".into());
        assert!(tool_call_timings(&rows)[0].asked_for_approval());
    }

    #[test]
    fn the_stalled_run_reports_open_calls_and_the_long_approval() {
        let mut meta = RunMeta::new("m", RunSource::Test);
        meta.started_at_ms = 0;
        let mut trace = RunTrace::from_parts(meta, Vec::new(), Vec::new());
        trace.records = stalled_batch();

        let health = check(&trace, 520_000, DEFAULT_SLOW_MS);
        assert!(!health.ended);
        assert_eq!(health.open_calls.len(), 2);
        assert_eq!(health.open_calls[0].tool_use_id, "toolu_x");
        assert_eq!(health.slow_approvals.len(), 1);
        assert_eq!(health.slow_approvals[0].wait_ms, 244_668);
        assert_eq!(health.unrecorded_results, ["toolu_a", "toolu_z"]);
        assert_eq!(health.problems.len(), 4, "{:#?}", health.problems);
        assert!(health.problems[0].starts_with("edit_file oolu_x has been waiting"));
        assert!(health.problems[2].contains("waited 4m 04s for approval"));

        let later = check(&trace, 245_687 + STALE_MS, DEFAULT_SLOW_MS);
        assert!(later
            .problems
            .iter()
            .any(|p| p.starts_with("run never ended")));
    }

    #[test]
    fn a_clean_run_is_healthy() {
        let mut meta = RunMeta::new("m", RunSource::Test);
        meta.ended_at_ms = Some(20);
        let mut trace = RunTrace::from_parts(meta, Vec::new(), Vec::new());
        trace.records = stalled_batch()[..3].to_vec();
        trace.steps = vec![crate::rewards::tests::result(
            1,
            0,
            "toolu_a",
            "edit_file",
            "x",
            "modify",
            false,
        )];
        let health = check(&trace, 1_000_000, DEFAULT_SLOW_MS);
        assert!(health.is_healthy(), "{:#?}", health.problems);
    }

    /// The same batch cancelled by the user: every call got a
    /// `[CANCELLED]` result, including the edit that had already run.
    #[test]
    fn a_cancelled_batch_says_which_calls_were_cut_off_and_which_ran() {
        let mut meta = RunMeta::new("m", RunSource::Test);
        meta.ended_at_ms = Some(600_000);
        let mut trace = RunTrace::from_parts(meta, Vec::new(), Vec::new());
        trace.records = stalled_batch();
        let cancelled = |seq, id: &str| {
            let mut step =
                crate::rewards::tests::result(seq, 0, id, "edit_file", "x", "modify", true);
            if let StepAction::ToolResult { excerpt, .. } = &mut step.action {
                *excerpt = "[CANCELLED] turn cancelled".into();
            }
            step
        };
        trace.steps = vec![
            crate::rewards::tests::result(1, 0, "toolu_a", "edit_file", "x", "modify", false),
            cancelled(2, "toolu_x"),
            cancelled(3, "toolu_y"),
            cancelled(4, "toolu_z"),
        ];

        let health = check(&trace, 600_000, DEFAULT_SLOW_MS);
        assert!(health.open_calls.iter().all(|c| c.cancelled));
        assert!(health.unrecorded_results.is_empty());
        assert_eq!(health.ran_but_reported_cancelled, ["toolu_z"]);
        assert!(health.problems[0].starts_with("edit_file oolu_x was cancelled"));
        assert!(health
            .problems
            .iter()
            .any(|p| p == "tool call oolu_z ran, but the model was told it was cancelled"));
    }
}
