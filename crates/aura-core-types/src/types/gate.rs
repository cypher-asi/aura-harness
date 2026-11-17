//! What Gate records about each prompt: its category and route on the
//! `UserPrompt` transaction ([`PromptPayload`]), and how the work went on
//! `Outcome` transactions ([`OutcomePayload`]) that refer back to it.
//!
//! A prompt can collect several outcomes: the one written when its turn
//! ends, then later signals (a rewind, a retry on a stronger model, the
//! user's rating). Which one counts is decided by whoever reads them; the
//! record keeps all of them.

use crate::ids::Hash;
use serde::{Deserialize, Serialize};

/// What a prompt asks for, as Gate's classifier labels it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptCategory {
    /// A quick question or explanation.
    Question,
    /// A small code change in one place.
    SmallChange,
    /// A change across several files.
    MultiFileChange,
    /// Fixing a failure.
    Debugging,
    /// Investigating a codebase or topic.
    Research,
    /// Designing or planning work.
    Planning,
    /// Prose, docs or messages.
    Writing,
    /// Anything else.
    Other,
    /// The classifier failed or gave no usable answer.
    Unclassified,
}

impl PromptCategory {
    /// The categories the classifier picks from, without
    /// [`Self::Unclassified`].
    pub const CLASSES: [Self; 8] = [
        Self::Question,
        Self::SmallChange,
        Self::MultiFileChange,
        Self::Debugging,
        Self::Research,
        Self::Planning,
        Self::Writing,
        Self::Other,
    ];

    /// The category's name as stored, e.g. `small_change`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Question => "question",
            Self::SmallChange => "small_change",
            Self::MultiFileChange => "multi_file_change",
            Self::Debugging => "debugging",
            Self::Research => "research",
            Self::Planning => "planning",
            Self::Writing => "writing",
            Self::Other => "other",
            Self::Unclassified => "unclassified",
        }
    }

    /// The category named `name` (as [`Self::as_str`] spells it).
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::CLASSES
            .into_iter()
            .chain([Self::Unclassified])
            .find(|c| c.as_str() == name)
    }
}

impl std::fmt::Display for PromptCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a routing decision covers.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RouteLevel {
    /// The main conversation loop for a whole turn.
    #[default]
    Turn,
    /// A piece of work delegated with the `task` tool.
    Subtask,
}

impl RouteLevel {
    /// The level's name as stored, e.g. `turn`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Turn => "turn",
            Self::Subtask => "subtask",
        }
    }
}

impl std::fmt::Display for RouteLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why the work runs on the model it runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteReason {
    /// The user (or an agent definition) chose the model.
    #[default]
    Pinned,
    /// Gate's default for the category.
    Default,
    /// The cheapest model that has proven itself on the category.
    Proven,
    /// Gate tried a cheaper model to learn how it does.
    Explore,
}

impl RouteReason {
    /// The reason's name as stored, e.g. `proven`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pinned => "pinned",
            Self::Default => "default",
            Self::Proven => "proven",
            Self::Explore => "explore",
        }
    }
}

impl std::fmt::Display for RouteReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The model a prompt's work runs on, and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Route {
    /// Whether this routes a turn or a subtask.
    pub level: RouteLevel,
    /// Model that runs the work, route prefix included.
    pub model: String,
    /// Why it runs there.
    pub reason: RouteReason,
    /// Whether Gate explored instead of taking its pick.
    pub explored: bool,
    /// Probability of this choice under Gate's policy, 1.0 when it was
    /// not a random choice.
    pub propensity: f64,
}

impl Route {
    /// A turn on `model`, chosen for `reason` without exploring.
    #[must_use]
    pub fn turn(model: impl Into<String>, reason: RouteReason) -> Self {
        Self {
            level: RouteLevel::Turn,
            model: model.into(),
            reason,
            explored: false,
            propensity: 1.0,
        }
    }

    /// A subtask on `model`, chosen for `reason` without exploring.
    #[must_use]
    pub fn subtask(model: impl Into<String>, reason: RouteReason) -> Self {
        Self {
            level: RouteLevel::Subtask,
            ..Self::turn(model, reason)
        }
    }
}

impl Default for Route {
    fn default() -> Self {
        Self::turn(String::new(), RouteReason::Pinned)
    }
}

/// Payload of a `UserPrompt` transaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptPayload {
    /// The prompt as typed (or as the harness wrote it).
    pub text: String,
    /// What the prompt asks for.
    pub category: PromptCategory,
    /// Model that classified the prompt; empty when nothing did.
    pub classifier: String,
    /// Where the prompt's work runs.
    pub route: Route,
    /// The kind of agent a subtask went to (the permanent agent the
    /// `task` call named); `None` for a turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

impl PromptPayload {
    /// `text`, not classified, with no route recorded.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            category: PromptCategory::Unclassified,
            classifier: String::new(),
            route: Route::default(),
            kind: None,
        }
    }

    /// The payload as stored on the transaction.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        to_json(self)
    }

    /// Read a stored payload.
    ///
    /// # Errors
    /// Returns the JSON error when `bytes` is not a prompt payload.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

/// What said how a prompt's work went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeSignal {
    /// The turn ended in an error.
    Error,
    /// The last test or build command of the turn failed.
    ChecksFailed,
    /// The last test or build command of the turn passed.
    ChecksPassed,
    /// The turn ended with nothing to judge it by.
    None,
    /// The user rewound the turn.
    Rewind,
    /// The user sent the same prompt again on a stronger model.
    RetryStronger,
    /// The user rated the turn good (`/good`).
    Good,
    /// The user rated the turn bad (`/bad`).
    Bad,
    /// The parent dispatched the same subtask again in the same turn.
    Redone,
}

impl OutcomeSignal {
    /// Whether the signal means the work succeeded; `None` when it says
    /// nothing either way.
    #[must_use]
    pub const fn success(self) -> Option<bool> {
        match self {
            Self::ChecksPassed | Self::Good => Some(true),
            Self::Error
            | Self::ChecksFailed
            | Self::Rewind
            | Self::RetryStronger
            | Self::Bad
            | Self::Redone => Some(false),
            Self::None => None,
        }
    }

    /// Whether this is the user's own rating, which overrides every
    /// automatic signal.
    #[must_use]
    pub const fn is_rating(self) -> bool {
        matches!(self, Self::Good | Self::Bad)
    }

    /// Whether this is written when the work ends, rather than later.
    #[must_use]
    pub const fn is_turn_end(self) -> bool {
        matches!(
            self,
            Self::Error | Self::ChecksFailed | Self::ChecksPassed | Self::None
        )
    }

    /// The signal's name as stored, e.g. `checks_failed`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::ChecksFailed => "checks_failed",
            Self::ChecksPassed => "checks_passed",
            Self::None => "none",
            Self::Rewind => "rewind",
            Self::RetryStronger => "retry_stronger",
            Self::Good => "good",
            Self::Bad => "bad",
            Self::Redone => "redone",
        }
    }
}

impl std::fmt::Display for OutcomeSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Payload of an `Outcome` transaction: how the work of the prompt
/// `prompt_tx` went.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutcomePayload {
    /// Hash of the prompt's `UserPrompt` transaction.
    #[serde(with = "crate::serde_helpers::hex_hash")]
    pub prompt_tx: Hash,
    /// The prompt's category.
    pub category: PromptCategory,
    /// Model that did the work.
    pub model: String,
    /// Whether the work succeeded; `None` when nothing said, which is not
    /// counted either way.
    pub success: Option<bool>,
    /// What said so.
    pub signal: OutcomeSignal,
    /// List-price cost of the work's model calls in USD.
    pub cost_usd: f64,
    /// Wall time of the work.
    pub latency_ms: u64,
}

impl OutcomePayload {
    /// The same work, judged by `signal` instead.
    #[must_use]
    pub fn with_signal(&self, signal: OutcomeSignal) -> Self {
        Self {
            success: signal.success(),
            signal,
            ..self.clone()
        }
    }

    /// The payload as stored on the transaction.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        to_json(self)
    }

    /// Read a stored payload.
    ///
    /// # Errors
    /// Returns the JSON error when `bytes` is not an outcome payload.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

fn to_json(value: &impl Serialize) -> Vec<u8> {
    // Strings, enums, booleans and numbers always serialize; an empty
    // payload would only come from a serde bug.
    serde_json::to_vec(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_payload_round_trips_with_string_forms() {
        let payload = PromptPayload {
            text: "fix the failing test".to_string(),
            category: PromptCategory::Debugging,
            classifier: "claude/claude-haiku-4-5".to_string(),
            route: Route::turn("claude/claude-sonnet-4-6", RouteReason::Default),
            kind: None,
        };
        let bytes = payload.to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["category"], "debugging");
        assert_eq!(json["route"]["level"], "turn");
        assert_eq!(json["route"]["reason"], "default");
        assert_eq!(json["route"]["propensity"], 1.0);
        assert!(json.get("kind").is_none(), "a turn names no kind");
        assert_eq!(PromptPayload::from_bytes(&bytes).unwrap(), payload);
    }

    #[test]
    fn subtask_prompt_payload_records_its_kind_and_level() {
        let payload = PromptPayload {
            category: PromptCategory::Research,
            route: Route::subtask("claude/claude-haiku-4-5", RouteReason::Proven),
            kind: Some("explorer".to_string()),
            ..PromptPayload::new("find the parser")
        };
        let bytes = payload.to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["route"]["level"], "subtask");
        assert_eq!(json["kind"], "explorer");
        assert_eq!(PromptPayload::from_bytes(&bytes).unwrap(), payload);
    }

    #[test]
    fn raw_text_is_not_a_prompt_payload() {
        assert!(PromptPayload::from_bytes(b"hello").is_err());
    }

    #[test]
    fn outcome_payload_round_trips() {
        let outcome = OutcomePayload {
            prompt_tx: Hash::from_content(b"prompt"),
            category: PromptCategory::SmallChange,
            model: "aura-claude-sonnet-4-6".to_string(),
            success: Some(false),
            signal: OutcomeSignal::ChecksFailed,
            cost_usd: 0.042,
            latency_ms: 12_345,
        };
        let bytes = outcome.to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["signal"], "checks_failed");
        assert_eq!(json["prompt_tx"], outcome.prompt_tx.to_hex());
        assert_eq!(OutcomePayload::from_bytes(&bytes).unwrap(), outcome);
    }

    #[test]
    fn signals_say_success_failure_or_nothing() {
        assert_eq!(OutcomeSignal::ChecksPassed.success(), Some(true));
        assert_eq!(OutcomeSignal::Good.success(), Some(true));
        for failure in [
            OutcomeSignal::Error,
            OutcomeSignal::ChecksFailed,
            OutcomeSignal::Rewind,
            OutcomeSignal::RetryStronger,
            OutcomeSignal::Bad,
            OutcomeSignal::Redone,
        ] {
            assert_eq!(failure.success(), Some(false), "{failure}");
        }
        assert_eq!(OutcomeSignal::None.success(), None);
        assert!(!OutcomeSignal::Redone.is_turn_end());
        assert_eq!(
            serde_json::to_value(OutcomeSignal::Redone).unwrap(),
            OutcomeSignal::Redone.as_str()
        );
    }

    #[test]
    fn with_signal_keeps_the_work_and_rejudges_it() {
        let outcome = OutcomePayload {
            prompt_tx: Hash::from_content(b"p"),
            category: PromptCategory::Question,
            model: "m".to_string(),
            success: None,
            signal: OutcomeSignal::None,
            cost_usd: 0.5,
            latency_ms: 10,
        };
        let good = outcome.with_signal(OutcomeSignal::Good);
        assert_eq!(good.success, Some(true));
        assert_eq!(good.signal, OutcomeSignal::Good);
        assert_eq!((good.cost_usd, good.latency_ms), (0.5, 10));
    }

    #[test]
    fn category_names_parse_back() {
        for category in PromptCategory::CLASSES
            .into_iter()
            .chain([PromptCategory::Unclassified])
        {
            assert_eq!(PromptCategory::from_name(category.as_str()), Some(category));
            let json = serde_json::to_value(category).unwrap();
            assert_eq!(json, category.as_str());
        }
        assert_eq!(PromptCategory::from_name("bugfix"), None);
    }

    #[test]
    fn level_and_reason_names_match_their_stored_form() {
        for level in [RouteLevel::Turn, RouteLevel::Subtask] {
            assert_eq!(serde_json::to_value(level).unwrap(), level.as_str());
        }
        for reason in [
            RouteReason::Pinned,
            RouteReason::Default,
            RouteReason::Proven,
            RouteReason::Explore,
        ] {
            assert_eq!(serde_json::to_value(reason).unwrap(), reason.as_str());
        }
    }
}
