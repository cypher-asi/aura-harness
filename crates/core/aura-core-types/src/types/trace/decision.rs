use super::step::StepLinks;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
