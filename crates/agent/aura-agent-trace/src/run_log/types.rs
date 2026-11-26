//! The types of the run log document.

use aura_core_types::trace::{FileTouch, HarnessAction, TokenUsage, ToolIntent};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::health::RunHealth;
use crate::scorecard::RunScorecard;

/// The whole log of a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunLog {
    /// [`LOG_SCHEMA`](super::LOG_SCHEMA); empty on nested logs.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schema: String,
    /// What the document holds; empty on nested logs.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub about: String,
    /// When the log was written.
    #[serde(default)]
    pub generated_at_ms: u64,
    /// Who ran what.
    pub run: LogRun,
    /// Where the run is stored.
    pub files: LogFiles,
    /// What the run did and cost, its subagents' cost included where said.
    pub totals: LogTotals,
    /// The run's scorecard, as in `summary.json`.
    pub scorecard: RunScorecard,
    /// Calls that never finished, slow approvals and other problems.
    pub health: RunHealth,
    /// Every turn in order.
    pub turns: Vec<LogTurn>,
    /// Logs of the runs this run spawned, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subagents: Vec<RunLog>,
    /// Secrets replaced in the whole document.
    #[serde(default)]
    pub redactions: usize,
}

/// Metadata of the run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogRun {
    pub run_id: String,
    /// Hex id of the agent whose conversation the run records.
    pub agent_id: Option<String>,
    pub title: String,
    /// Model requested for the run; each call names the model that served it.
    pub model: String,
    pub mode: String,
    /// `tui`, `gateway`, `eval`, ...
    pub source: String,
    pub task_id: Option<String>,
    pub workspace: String,
    /// `decisions` or `full`; at `full`, tool outputs are complete.
    pub trace_level: String,
    pub harness_version: String,
    pub started_at_ms: u64,
    pub ended_at_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    /// The run and call that spawned this one.
    pub spawned_by: Option<LogSpawn>,
    /// Run this one was branched from, and the step it starts after.
    pub parent_run: Option<String>,
    pub branch_step: Option<u64>,
    /// Eval arm or experiment label.
    pub arm: Option<String>,
    /// Harness thresholds in effect.
    pub params: Value,
}

/// Where a spawned run came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogSpawn {
    /// The spawning run.
    pub run_id: String,
    /// The spawning call, when it could be told.
    pub tool_use_id: Option<String>,
    /// Turn and step of the spawning run that made the call.
    pub turn: Option<u32>,
    pub step: Option<u64>,
}

/// Files of the run's bundle, as absolute paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogFiles {
    /// The bundle directory.
    pub bundle: String,
    /// This document.
    pub log: String,
    pub meta: String,
    /// The raw steps, one JSON object per line.
    pub steps: String,
    /// Kernel record rows joined to the steps.
    pub record: String,
    pub rewards: String,
    pub summary: String,
    /// Blobs of full requests and outputs, when the run kept them.
    pub blobs: Option<String>,
}

/// Counts and cost.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LogTotals {
    pub turns: usize,
    pub model_calls: usize,
    pub tool_calls: usize,
    pub tool_errors: usize,
    pub decisions: usize,
    pub usage: TokenUsage,
    /// Estimated cost of this run's model calls.
    pub cost_usd: Option<f64>,
    /// Runs below this one, at any depth.
    pub subagents: usize,
    /// Estimated cost of this run and every run below it.
    pub cost_usd_with_subagents: Option<f64>,
}

/// One user turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogTurn {
    /// 1-based.
    pub turn: u32,
    /// What the user asked; `None` for a turn the log has no prompt of.
    pub prompt: Option<String>,
    pub started_at_ms: u64,
    pub ended_at_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub model_calls: usize,
    pub tool_calls: usize,
    pub tool_errors: usize,
    pub decisions: usize,
    pub usage: TokenUsage,
    pub cost_usd: Option<f64>,
    /// Why the turn stopped; `None` while it runs.
    pub stop_reason: Option<String>,
    /// Why its last model call failed, when one did.
    pub error: Option<String>,
    pub files_changed: Option<usize>,
    /// Runs spawned in the turn.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subagents: Vec<String>,
    pub steps: Vec<LogStep>,
}

/// One step of a turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogStep {
    /// Position in the run, from 1.
    pub seq: u64,
    /// The model call a tool result answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u64>,
    /// Zero-based model call within the turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration: Option<u32>,
    pub at_ms: u64,
    /// `user`, `model`, `environment` or `harness/<source>`.
    pub actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(flatten)]
    pub event: LogEvent,
    /// Rows of the kernel record log the step matched.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub record_seqs: Vec<u64>,
    /// Steps this one was caused by, as the nudge before a call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub caused_by: Vec<u64>,
}

/// What happened at a step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LogEvent {
    UserPrompt {
        text: String,
    },
    ModelCall {
        /// Model that served the call.
        model: String,
        /// What the harness made the call for (`classify`, `criteria`,
        /// `judge`); `None` for the agent's own calls.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        purpose: Option<String>,
        /// The model's reasoning; `None` when it showed none or the run
        /// did not keep it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking: Option<String>,
        thinking_chars: usize,
        /// The model reasoned but the run kept only how much.
        #[serde(default, skip_serializing_if = "is_false")]
        thinking_omitted: bool,
        /// Visible reply text.
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<LogToolCall>,
        stop_reason: String,
        /// Why the call failed, when it did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        usage: TokenUsage,
        cost_usd: Option<f64>,
    },
    ToolResult {
        tool_use_id: String,
        tool: String,
        is_error: bool,
        /// `executor`, `cache` or `blocked`.
        source: String,
        /// Length of the whole output.
        content_chars: usize,
        /// The output: whole when `output_complete`, else its start.
        output: String,
        output_complete: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files: Vec<FileTouch>,
    },
    Decision {
        /// Harness component (`steering`, `compaction`, `retry`, ...).
        source: String,
        name: String,
        action: HarnessAction,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        alternatives: Vec<String>,
        /// The condition that fired.
        #[serde(default, skip_serializing_if = "Value::is_null")]
        trigger: Value,
        /// The state the heuristic looked at.
        #[serde(default, skip_serializing_if = "Value::is_null")]
        features: Value,
    },
    TurnEnd {
        stop_reason: String,
        iterations: usize,
        input_tokens: u64,
        output_tokens: u64,
        files_changed: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

/// A tool call a model made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogToolCall {
    pub tool_use_id: String,
    pub name: String,
    pub intent: ToolIntent,
    pub input: Value,
    /// Run of the agent the call spawned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_run: Option<String>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(b: &bool) -> bool {
    !*b
}

impl RunLog {
    /// The run's turn `turn`.
    #[must_use]
    pub fn turn(&self, turn: u32) -> Option<&LogTurn> {
        self.turns.iter().find(|t| t.turn == turn)
    }

    /// This log or one nested in it, by run id.
    #[must_use]
    pub fn find(&self, run_id: &str) -> Option<&Self> {
        if self.run.run_id == run_id {
            return Some(self);
        }
        self.subagents.iter().find_map(|child| child.find(run_id))
    }
}
