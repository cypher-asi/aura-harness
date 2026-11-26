use super::decision::{DecisionSource, HarnessAction};
use super::model::{ModelAction, ModelObservation, TokenUsage};
use super::BlobId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Who decided at a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Actor {
    /// The human driving the session.
    User,
    /// The language model.
    Model,
    /// A harness heuristic.
    Harness {
        /// Component that decided.
        source: DecisionSource,
    },
    /// The workspace responding to an action (tool results).
    Environment,
}

impl Actor {
    /// Short label for display.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Model => "model",
            Self::Harness { source } => source.as_str(),
            Self::Environment => "environment",
        }
    }
}

/// What the decider saw.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Observation {
    /// Nothing recorded.
    #[default]
    None,
    /// Exact model request.
    Model(ModelObservation),
    /// Heuristic features and trigger.
    Harness {
        /// Typed state the heuristic looked at.
        #[serde(default)]
        features: Value,
        /// The condition that fired.
        #[serde(default)]
        trigger: Value,
    },
}

/// A file a tool touched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTouch {
    /// Workspace-relative path.
    pub path: String,
    /// `read`, `create`, `modify` or `delete`.
    pub kind: String,
    /// Lines added, when known.
    #[serde(default)]
    pub added: u32,
    /// Lines removed, when known.
    #[serde(default)]
    pub removed: u32,
}

/// What happened at a step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StepAction {
    /// The user sent a prompt.
    UserPrompt {
        /// Prompt text.
        text: String,
    },
    /// The model answered.
    Model(ModelAction),
    /// A harness heuristic acted.
    Harness {
        /// Heuristic name.
        name: String,
        /// What it did.
        action: HarnessAction,
        /// Enumerable alternatives.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        alternatives: Vec<String>,
    },
    /// A tool returned.
    ToolResult {
        /// Call this result answers.
        tool_use_id: String,
        /// Tool name.
        tool: String,
        /// Whether the call failed.
        is_error: bool,
        /// Who produced the result: `executor`, `cache` or `blocked`.
        #[serde(default)]
        source: String,
        /// Result length in characters.
        content_chars: usize,
        /// First part of the result.
        #[serde(default)]
        excerpt: String,
        /// Full result, when blobs are recorded.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<BlobId>,
        /// Files read or written.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files: Vec<FileTouch>,
    },
    /// A turn finished.
    TurnEnd {
        /// Why the loop stopped.
        stop_reason: String,
        /// Model calls in the turn.
        iterations: usize,
        /// Prompt tokens in the turn.
        input_tokens: u64,
        /// Generated tokens in the turn.
        output_tokens: u64,
        /// Files changed in the turn.
        files_changed: usize,
        /// Why the last model call failed, when one did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

impl StepAction {
    /// Short discriminator matching the serde tag.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::UserPrompt { .. } => "user_prompt",
            Self::Model(_) => "model",
            Self::Harness { .. } => "harness",
            Self::ToolResult { .. } => "tool_result",
            Self::TurnEnd { .. } => "turn_end",
        }
    }
}

/// Kind of policy behind an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PolicyKind {
    /// A language model.
    Model,
    /// A harness heuristic.
    Harness,
    /// A human.
    Human,
    /// The environment (tools).
    #[default]
    Environment,
}

/// Which policy produced an action, with enough detail to pool or
/// separate actions across versions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PolicyRef {
    /// Policy kind.
    pub kind: PolicyKind,
    /// Model id or heuristic name.
    pub id: String,
    /// Heuristic version or harness git sha.
    #[serde(default)]
    pub version: String,
    /// Sampling or heuristic parameters.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
    /// Probability the chosen action had under a randomized policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub propensity: Option<f64>,
}

/// Join keys from a step into the record log and workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StepLinks {
    /// Provider tool-use ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_use_ids: Vec<String>,
    /// Record-log sequence numbers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub record_seqs: Vec<u64>,
    /// Workspace-relative file paths.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// Steps this one was caused by (e.g. the nudge before an edit).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub caused_by: Vec<u64>,
}

impl StepLinks {
    /// Whether every list is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tool_use_ids.is_empty()
            && self.record_seqs.is_empty()
            && self.files.is_empty()
            && self.caused_by.is_empty()
    }
}

/// One step of a run trajectory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    /// Run this step belongs to.
    pub run_id: String,
    /// Monotonic position within the run, starting at 1.
    pub seq: u64,
    /// Enclosing step (the model call a tool result answers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u64>,
    /// 1-based turn.
    pub turn: u32,
    /// Zero-based iteration within the turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration: Option<u32>,
    /// Unix milliseconds.
    pub at_ms: u64,
    /// Who decided.
    pub actor: Actor,
    /// What they saw.
    #[serde(default)]
    pub observation: Observation,
    /// What happened.
    pub action: StepAction,
    /// Which policy acted.
    #[serde(default)]
    pub policy: PolicyRef,
    /// Join keys.
    #[serde(default, skip_serializing_if = "StepLinks::is_empty")]
    pub links: StepLinks,
    /// Token accounting for model steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    /// Wall time for model and tool steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}
