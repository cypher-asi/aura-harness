//! How a prompt's work went: the payload of an `Outcome` transaction, and
//! what said so.

use super::category::PromptCategory;
use super::to_json;
use crate::ids::Hash;
use serde::{Deserialize, Serialize};

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
    /// The user stopped the work, or their sign-in was refused, before
    /// it ended. It says nothing about the model's work.
    Cancelled,
    /// The user rewound the turn.
    Rewind,
    /// The user sent the same prompt again on a stronger model.
    RetryStronger,
    /// The parent dispatched the same subtask again in the same turn.
    Redone,
    /// The judge model reviewed work that ran no check and found it done.
    JudgedPass,
    /// The judge model reviewed the work and found the model at fault:
    /// its work was wrong or incomplete.
    JudgedFail,
    /// The judge model reviewed the failed work and found the failure was
    /// not the model's doing (the environment, a check that already
    /// failed, a request that can't be done). The work then counts neither
    /// for nor against the model.
    JudgedExcused,
    /// The work ended because a model it needed could not be reached or
    /// refused the request: the provider was overloaded or rate-limiting,
    /// the connection failed, the stream went silent, or the sign-in was
    /// refused; or the judge that gives work with no check its verdict
    /// gave none. It says nothing about the model's work, so like
    /// [`Self::JudgedExcused`] it counts neither way.
    Unavailable,
}

impl OutcomeSignal {
    /// Whether the signal means the work succeeded; `None` when it says
    /// nothing either way.
    #[must_use]
    pub const fn success(self) -> Option<bool> {
        match self {
            Self::ChecksPassed | Self::JudgedPass => Some(true),
            Self::Error
            | Self::ChecksFailed
            | Self::Rewind
            | Self::RetryStronger
            | Self::Redone
            | Self::JudgedFail => Some(false),
            Self::Cancelled | Self::JudgedExcused | Self::Unavailable => None,
        }
    }

    /// Whether the signal takes back what earlier signals said about the
    /// work, so that it counts neither as a success nor as a failure.
    #[must_use]
    pub const fn voids(self) -> bool {
        matches!(self, Self::JudgedExcused | Self::Unavailable)
    }

    /// Whether this is written when the work ends, rather than later.
    #[must_use]
    pub const fn is_turn_end(self) -> bool {
        matches!(
            self,
            Self::Error
                | Self::ChecksFailed
                | Self::ChecksPassed
                | Self::Cancelled
                | Self::JudgedPass
                | Self::JudgedFail
                | Self::Unavailable
        )
    }

    /// The signal's name as stored, e.g. `checks_failed`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::ChecksFailed => "checks_failed",
            Self::ChecksPassed => "checks_passed",
            Self::Cancelled => "cancelled",
            Self::Rewind => "rewind",
            Self::RetryStronger => "retry_stronger",
            Self::Redone => "redone",
            Self::JudgedPass => "judged_pass",
            Self::JudgedFail => "judged_fail",
            Self::JudgedExcused => "judged_excused",
            Self::Unavailable => "unavailable",
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
    /// List-price cost of the model calls of the subtasks the work
    /// dispatched, in USD; not part of `cost_usd`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub subtask_cost_usd: f64,
    /// Wall time of the work.
    pub latency_ms: u64,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(cost: &f64) -> bool {
    *cost == 0.0
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
