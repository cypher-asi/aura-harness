//! Run trajectory schema.
//!
//! A coding run is recorded as an ordered list of [`Step`]s. Each step
//! names who decided ([`Actor`]), what they saw ([`Observation`]), what
//! they chose ([`StepAction`]) and which policy chose it
//! ([`PolicyRef`]). [`Reward`]s are stored separately and point back at
//! a step or the whole run, so labels can arrive after the run without
//! rewriting it.
//!
//! The same shapes feed run scorecards, the terminal graph view, eval
//! comparisons and training-dataset exports. Bump
//! [`TRACE_SCHEMA_VERSION`] on any change that old readers would
//! misinterpret.
//!
//! Large observation payloads (system prompt, tool schemas, message
//! history, thinking) are stored out of line as content-addressed
//! blobs and referenced by [`BlobId`].

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Version of the on-disk trajectory format.
pub const TRACE_SCHEMA_VERSION: u32 = 1;

/// Hex-encoded BLAKE3 digest of a blob's bytes.
pub type BlobId = String;

/// Content address for `bytes`.
#[must_use]
pub fn blob_id(bytes: &[u8]) -> BlobId {
    blake3::hash(bytes).to_hex().to_string()
}

/// Harness component that made a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionSource {
    /// Steering nudges injected into the next request.
    Steering,
    /// Context compaction before a model call.
    Compaction,
    /// Iteration / credit budget warnings and stops.
    Budget,
    /// Pre-dispatch tool gates (exploration block, failed-edit guard).
    ToolPipeline,
    /// Tool-result cache hits.
    Cache,
    /// Kernel policy verdicts.
    Policy,
    /// Provider retries and stream resets.
    Retry,
    /// Automatic build checks after writes.
    Build,
    /// Completion guards (self-review, `task_done` gates).
    SelfReview,
    /// Reasoning-effort and output-budget choices.
    Thinking,
    /// Loop termination.
    Stop,
    /// Gate: which model a prompt's work goes to, and the calls it makes
    /// to classify, write criteria for and judge that work.
    Gate,
}

impl DecisionSource {
    /// Every source, in declaration order.
    pub const ALL: [Self; 12] = [
        Self::Steering,
        Self::Compaction,
        Self::Budget,
        Self::ToolPipeline,
        Self::Cache,
        Self::Policy,
        Self::Retry,
        Self::Build,
        Self::SelfReview,
        Self::Thinking,
        Self::Stop,
        Self::Gate,
    ];

    /// Stable snake-case name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Steering => "steering",
            Self::Compaction => "compaction",
            Self::Budget => "budget",
            Self::ToolPipeline => "tool_pipeline",
            Self::Cache => "cache",
            Self::Policy => "policy",
            Self::Retry => "retry",
            Self::Build => "build",
            Self::SelfReview => "self_review",
            Self::Thinking => "thinking",
            Self::Stop => "stop",
            Self::Gate => "gate",
        }
    }
}

/// What the harness did at a decision point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HarnessAction {
    /// Injected guidance text into the next request.
    Nudge {
        /// Short label of the injected message kind.
        kind: String,
    },
    /// Refused a tool call and returned a synthetic error instead.
    Block {
        /// Tool that was refused.
        tool: String,
        /// Refused call, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_use_id: Option<String>,
        /// Why it was refused.
        reason: String,
    },
    /// Considered intervening and let the call proceed.
    Allow,
    /// Rewrote message history to fit the context window.
    Compact {
        /// Tier or strategy that ran.
        tier: String,
        /// Estimated context tokens before compaction.
        tokens_before: u64,
        /// Estimated context tokens after compaction.
        tokens_after: u64,
    },
    /// Served a tool call from the result cache instead of running it.
    CacheHit {
        /// Cached tool.
        tool: String,
        /// Served call.
        tool_use_id: String,
    },
    /// Retried a provider call after a transient failure.
    Retry {
        /// 1-based attempt that will now run.
        attempt: u32,
        /// Backoff before the attempt.
        wait_ms: u64,
        /// Error class.
        reason: String,
    },
    /// Appended a warning to the model context.
    Warn {
        /// Warning text.
        message: String,
    },
    /// Ran a build check.
    Build {
        /// Whether the build passed.
        success: bool,
        /// Reported error count.
        error_count: usize,
    },
    /// Ended the loop.
    Stop {
        /// Stop reason.
        reason: String,
    },
    /// Sent the work to a model.
    Route {
        /// Model the work went to.
        model: String,
        /// Reasoning effort it was given, when one was chosen.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
    },
}

impl HarnessAction {
    /// Short discriminator matching the serde tag.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Nudge { .. } => "nudge",
            Self::Block { .. } => "block",
            Self::Allow => "allow",
            Self::Compact { .. } => "compact",
            Self::CacheHit { .. } => "cache_hit",
            Self::Retry { .. } => "retry",
            Self::Warn { .. } => "warn",
            Self::Build { .. } => "build",
            Self::Stop { .. } => "stop",
            Self::Route { .. } => "route",
        }
    }
}

/// One harness decision, as emitted by the agent loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessDecision {
    /// Component that decided.
    pub source: DecisionSource,
    /// Heuristic name, e.g. `implement_now` or `failed_edit_guard`.
    pub name: String,
    /// Implementation version of the heuristic. Bump when its logic
    /// changes so old and new decisions are not pooled together.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Zero-based iteration the decision applies to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration: Option<u32>,
    /// What the harness did.
    pub action: HarnessAction,
    /// Typed state the heuristic looked at.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub features: Value,
    /// The specific condition that fired.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub trigger: Value,
    /// Other actions the heuristic could have taken, when enumerable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<String>,
    /// Join keys into the record log and workspace.
    #[serde(default, skip_serializing_if = "StepLinks::is_empty")]
    pub links: StepLinks,
}

const fn default_version() -> u32 {
    1
}

impl HarnessDecision {
    /// Build a decision with empty features, trigger and links.
    #[must_use]
    pub fn new(source: DecisionSource, name: impl Into<String>, action: HarnessAction) -> Self {
        Self {
            source,
            name: name.into(),
            version: 1,
            iteration: None,
            action,
            features: Value::Null,
            trigger: Value::Null,
            alternatives: Vec::new(),
            links: StepLinks::default(),
        }
    }

    /// Set the iteration.
    #[must_use]
    pub fn at_iteration(mut self, iteration: usize) -> Self {
        self.iteration = u32::try_from(iteration).ok();
        self
    }

    /// Set the features.
    #[must_use]
    pub fn with_features(mut self, features: Value) -> Self {
        self.features = features;
        self
    }

    /// Set the trigger.
    #[must_use]
    pub fn with_trigger(mut self, trigger: Value) -> Self {
        self.trigger = trigger;
        self
    }

    /// Set the alternatives.
    #[must_use]
    pub fn with_alternatives(mut self, alternatives: &[&str]) -> Self {
        self.alternatives = alternatives.iter().map(|s| (*s).to_string()).collect();
        self
    }

    /// Link to tool calls.
    #[must_use]
    pub fn with_tool_use(mut self, tool_use_id: impl Into<String>) -> Self {
        self.links.tool_use_ids.push(tool_use_id.into());
        self
    }

    /// Link to files.
    #[must_use]
    pub fn with_file(mut self, path: impl Into<String>) -> Self {
        self.links.files.push(path.into());
        self
    }
}

/// What the model saw for one call. Every field except `params` is a
/// blob reference so the exact request can be rebuilt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelObservation {
    /// System prompt.
    pub system: BlobId,
    /// Tool definitions, serialized as a JSON array.
    pub tools: BlobId,
    /// One blob per message, in order.
    pub messages: Vec<BlobId>,
    /// Sampling parameters (model, max tokens, temperature, thinking).
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

/// Coarse purpose of a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolIntent {
    /// Reading or searching the workspace.
    Explore,
    /// Mutating files.
    Edit,
    /// Building, testing or linting.
    Verify,
    /// Planning or task bookkeeping.
    Plan,
    /// Starting or messaging other agents.
    Delegate,
    /// Declaring the task finished.
    Complete,
    /// Anything else (arbitrary commands, network tools).
    Other,
}

impl ToolIntent {
    /// Classify a tool call by name and input.
    #[must_use]
    pub fn classify(name: &str, input: &Value) -> Self {
        match name {
            "read_file" | "list_files" | "search_code" | "find_files" | "stat_file" | "glob"
            | "grep" | "web_search" | "web_fetch" | "get_task_context" => Self::Explore,
            "write_file" | "edit_file" | "delete_file" | "apply_patch" | "multi_edit" => Self::Edit,
            "submit_plan" | "todo_write" | "update_plan" => Self::Plan,
            "task" | "spawn_agent" | "delegate_task" | "send_to_agent" | "agent_lifecycle" => {
                Self::Delegate
            }
            "task_done" => Self::Complete,
            "run_command" => {
                let command = input
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if is_verify_command(&command) {
                    Self::Verify
                } else {
                    Self::Other
                }
            }
            _ => Self::Other,
        }
    }

    /// Stable snake-case name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Explore => "explore",
            Self::Edit => "edit",
            Self::Verify => "verify",
            Self::Plan => "plan",
            Self::Delegate => "delegate",
            Self::Complete => "complete",
            Self::Other => "other",
        }
    }
}

fn is_verify_command(command: &str) -> bool {
    const MARKERS: [&str; 14] = [
        "test", "build", "check", "clippy", "lint", "pytest", "tsc", "make", "fmt", "vitest",
        "jest", "mypy", "eslint", "ruff",
    ];
    MARKERS.iter().any(|m| command.contains(m))
}

/// One tool call the model asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallAction {
    /// Provider tool-use id.
    pub tool_use_id: String,
    /// Tool name.
    pub name: String,
    /// Parsed input.
    pub input: Value,
    /// Classified purpose.
    pub intent: ToolIntent,
}

/// What the model produced for one call.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ModelAction {
    /// Full assistant message, when blobs are recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<BlobId>,
    /// Thinking text, when blobs are recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<BlobId>,
    /// Length of the thinking text in characters.
    #[serde(default)]
    pub thinking_chars: usize,
    /// Thinking text, kept at every capture level. Empty for runs
    /// recorded before it was kept, and for models that show none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thinking_text: String,
    /// Visible assistant text.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// Requested tool calls, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallAction>,
    /// Provider stop reason.
    #[serde(default)]
    pub stop_reason: String,
    /// What the harness made the call for (`classify`, `criteria`,
    /// `judge`); `None` for the agent's own calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// Why the call failed, when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Token accounting for one model call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Uncached prompt tokens.
    pub input: u64,
    /// Generated tokens.
    pub output: u64,
    /// Prompt tokens written to the cache.
    #[serde(default)]
    pub cache_write: u64,
    /// Prompt tokens read from the cache.
    #[serde(default)]
    pub cache_read: u64,
}

/// One complete model call, as emitted by the agent loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelExchange {
    /// Zero-based iteration.
    pub iteration: u32,
    /// Model that served the call.
    pub model: String,
    /// What the model saw. `None` when blob capture is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<ModelObservation>,
    /// What the model produced.
    pub action: ModelAction,
    /// Token accounting.
    pub usage: TokenUsage,
    /// Wall time of the call.
    pub latency_ms: u64,
    /// Provider correlation id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_request_id: Option<String>,
}

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

/// What a reward scores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum RewardTarget {
    /// One step.
    Step {
        /// Step sequence number.
        seq: u64,
    },
    /// The whole run.
    Run,
}

/// Kind of reward signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewardKind {
    /// Task verification result.
    Outcome,
    /// Local progress signal (edit applied, build fixed, redundant read).
    Process,
    /// Token, money or time spent.
    Cost,
    /// Whether a harness decision changed behavior.
    DecisionEffect,
    /// Explicit human label.
    HumanExplicit,
    /// Inferred from human behavior (revert, interrupt, commit).
    HumanImplicit,
}

impl RewardKind {
    /// Stable snake-case name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Outcome => "outcome",
            Self::Process => "process",
            Self::Cost => "cost",
            Self::DecisionEffect => "decision_effect",
            Self::HumanExplicit => "human_explicit",
            Self::HumanImplicit => "human_implicit",
        }
    }
}

/// A scalar label on a step or run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reward {
    /// What is scored.
    pub target: RewardTarget,
    /// Signal kind.
    pub kind: RewardKind,
    /// Short name of the specific signal, e.g. `edit_applied`.
    pub name: String,
    /// Score; positive is good.
    pub value: f64,
    /// Labeler that produced it.
    pub source: String,
    /// Unix milliseconds.
    pub at_ms: u64,
    /// Supporting data.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub evidence: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn harness_decision_roundtrips() {
        let decision = HarnessDecision::new(
            DecisionSource::Steering,
            "implement_now",
            HarnessAction::Nudge {
                kind: "implement_now".into(),
            },
        )
        .at_iteration(4)
        .with_features(json!({"exploration_count": 11}))
        .with_alternatives(&["allow"]);
        let text = serde_json::to_string(&decision).unwrap();
        let back: HarnessDecision = serde_json::from_str(&text).unwrap();
        assert_eq!(decision, back);
        assert!(text.contains("\"type\":\"nudge\""));
    }

    #[test]
    fn step_roundtrips_with_nested_tags() {
        let step = Step {
            run_id: "r1".into(),
            seq: 3,
            parent: Some(2),
            turn: 1,
            iteration: Some(0),
            at_ms: 10,
            actor: Actor::Harness {
                source: DecisionSource::Cache,
            },
            observation: Observation::Harness {
                features: json!({"hits": 1}),
                trigger: Value::Null,
            },
            action: StepAction::Harness {
                name: "tool_result_cache".into(),
                action: HarnessAction::CacheHit {
                    tool: "read_file".into(),
                    tool_use_id: "t1".into(),
                },
                alternatives: vec![],
            },
            policy: PolicyRef {
                kind: PolicyKind::Harness,
                id: "tool_result_cache".into(),
                version: "1".into(),
                params: Value::Null,
                propensity: Some(0.5),
            },
            links: StepLinks::default(),
            usage: None,
            duration_ms: None,
        };
        let text = serde_json::to_string(&step).unwrap();
        let back: Step = serde_json::from_str(&text).unwrap();
        assert_eq!(step, back);
    }

    #[test]
    fn reward_roundtrips() {
        let reward = Reward {
            target: RewardTarget::Step { seq: 7 },
            kind: RewardKind::Process,
            name: "edit_applied".into(),
            value: 0.2,
            source: "process".into(),
            at_ms: 1,
            evidence: Value::Null,
        };
        let back: Reward = serde_json::from_str(&serde_json::to_string(&reward).unwrap()).unwrap();
        assert_eq!(reward, back);
    }

    #[test]
    fn classifies_tool_intents() {
        assert_eq!(
            ToolIntent::classify("read_file", &json!({})),
            ToolIntent::Explore
        );
        assert_eq!(
            ToolIntent::classify("edit_file", &json!({})),
            ToolIntent::Edit
        );
        assert_eq!(
            ToolIntent::classify("run_command", &json!({"command": "cargo test -p x"})),
            ToolIntent::Verify
        );
        assert_eq!(
            ToolIntent::classify("run_command", &json!({"command": "ls -la"})),
            ToolIntent::Other
        );
        assert_eq!(
            ToolIntent::classify("task", &json!({})),
            ToolIntent::Delegate
        );
        assert_eq!(
            ToolIntent::classify("task_done", &json!({})),
            ToolIntent::Complete
        );
    }

    #[test]
    fn blob_id_is_stable_hex() {
        let id = blob_id(b"hello");
        assert_eq!(id.len(), 64);
        assert_eq!(id, blob_id(b"hello"));
        assert_ne!(id, blob_id(b"hello!"));
    }
}
