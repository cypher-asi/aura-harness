use std::path::PathBuf;

use super::super::gate::{Assessment, Difficulty, OutcomeSignal, PromptCategory};
use super::super::plan_usage::PlanShare;
use super::arm::CompareArm;
use crate::ids::{AgentId, Hash};
use serde::{Deserialize, Serialize};

/// Tokens and list-price cost one model used in a run, for the run's
/// own work or for one purpose beside it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelTokens {
    /// Model as the provider named it.
    pub model: String,
    /// What the calls were made for beside the run's own work
    /// (`classify`, `criteria`, `judge`, `title`); `None` for the work
    /// itself, and for every call in records written before calls were
    /// told apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// Model calls.
    pub calls: u32,
    /// Prompt tokens, cached ones included.
    pub input_tokens: u64,
    /// Prompt tokens read from the cache.
    pub cache_read_tokens: u64,
    /// Prompt tokens written to the cache.
    pub cache_write_tokens: u64,
    /// Completion tokens.
    pub output_tokens: u64,
    /// List-price cost in USD of the priced calls: those of `calls` not
    /// in `unpriced_calls`. Local models are priced, at 0.
    pub cost_usd: f64,
    /// Calls with no known list price, which `cost_usd` leaves out; 0 in
    /// records written before they were counted.
    #[serde(default, skip_serializing_if = "is_zero_calls")]
    pub unpriced_calls: u32,
}

impl ModelTokens {
    /// Whether some of its calls have no known price, so `cost_usd` is
    /// only what the others cost.
    #[must_use]
    pub const fn is_incomplete(&self) -> bool {
        self.unpriced_calls > 0
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
pub(super) const fn is_zero_calls(calls: &u32) -> bool {
    *calls == 0
}

/// Where a run's workspace copy is and what it started from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompareCopy {
    /// Root of the copy: a detached git worktree, or a plain copy.
    pub root: PathBuf,
    /// The folder the run's agent works in (the user's folder inside
    /// the copy).
    pub workdir: PathBuf,
    /// Git directory kept beside a plain copy to diff it; `None` for a
    /// worktree, which has its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_dir: Option<PathBuf>,
    /// Git tree of the copy before the run, which its diff is taken
    /// against.
    pub baseline: String,
}

/// One prompt of a run: what it cost and how it ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunTurn {
    /// Model that ran the turn.
    pub model: String,
    /// List-price cost of the turn's model calls, subtasks included.
    pub cost_usd: f64,
    /// Wall time of the turn.
    pub wall_ms: u64,
    /// How the turn ended, as its `Outcome` records it.
    pub signal: OutcomeSignal,
    /// What its prompt asked for; `None` in older records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<PromptCategory>,
    /// How hard the classifier judged it, when it said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<Difficulty>,
    /// What the classifier found about its work, which `difficulty` was
    /// worked out from; `None` when it named the difficulty itself, and
    /// in older records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assessment: Option<Assessment>,
    /// Whether the agent led it, handing its work to subtasks; `None` in
    /// older records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led: Option<bool>,
    /// Times a failed attempt moved it to a stronger model.
    #[serde(default)]
    pub escalations: u32,
    /// The model it was first sent to, when a failed attempt moved it on
    /// from there to `model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_model: Option<String>,
    /// Reasoning effort Gate ran `model` at (`high`, `max`, ...); `None`
    /// when it chose none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// The effort it was first run at, when a failed attempt raised it
    /// from there to `effort`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_effort: Option<String>,
    /// Times Gate asked the model to check a change it had not checked.
    #[serde(default)]
    pub nudges: u32,
    /// Times Gate sent the work back to the model that did it, with what
    /// a review found unfinished or the signs of failure it showed.
    #[serde(default)]
    pub sent_back: u32,
    /// The part of `cost_usd` its subtasks spent.
    #[serde(default)]
    pub subtask_cost_usd: f64,
    /// Its model calls, subtasks included, with no known list price,
    /// which `cost_usd` leaves out; 0 in older records.
    #[serde(default, skip_serializing_if = "is_zero_calls")]
    pub unpriced_calls: u32,
}

impl RunTurn {
    /// A turn on `model` that cost `cost_usd`, took `wall_ms` and ended
    /// with `signal`, with nothing known of how it was routed.
    #[must_use]
    pub fn new(
        model: impl Into<String>,
        cost_usd: f64,
        wall_ms: u64,
        signal: OutcomeSignal,
    ) -> Self {
        Self {
            model: model.into(),
            cost_usd,
            wall_ms,
            signal,
            category: None,
            difficulty: None,
            assessment: None,
            led: None,
            escalations: 0,
            first_model: None,
            effort: None,
            first_effort: None,
            nudges: 0,
            sent_back: 0,
            subtask_cost_usd: 0.0,
            unpriced_calls: 0,
        }
    }
}

/// One run of a comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompareRun {
    /// `a-1`, `b-2`, ... (`gate-1`, `other-2` in older records)
    pub name: String,
    /// The arm it belongs to.
    pub arm: CompareArm,
    /// The agent that ran it.
    pub agent_id: AgentId,
    /// Its first prompt transaction, once recorded.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_helpers::option_hex_hash"
    )]
    pub prompt_tx: Option<Hash>,
    /// Model that ran the first turn.
    pub model: String,
    /// Its workspace copy.
    pub copy: CompareCopy,
    /// Wall time from the first prompt to the end of the last turn,
    /// subtasks included.
    pub wall_ms: u64,
    /// List-price cost of every priced model call: the turns, their
    /// classifier, and the subtasks they dispatched. Calls with no known
    /// price are left out and counted in `tokens`.
    pub cost_usd: f64,
    /// Tokens and cost per model and purpose, most expensive first.
    pub tokens: Vec<ModelTokens>,
    /// Each prompt's turn, in order; empty in older records.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turns: Vec<RunTurn>,
    /// Share of each plan window the run used, when its calls ran on a
    /// plan whose provider reports usage.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plan_usage: Vec<PlanShare>,
    /// Whether the check command passed in the copy; `None` without a
    /// check, or when it could not run.
    pub check_passed: Option<bool>,
    /// Files the run changed.
    pub files_changed: u32,
    /// Why the run did not finish, if it did not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Id of the example task it worked on, in a benchmark of example
    /// tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

impl CompareRun {
    /// Its model calls with no known list price, which `cost_usd` leaves
    /// out.
    #[must_use]
    pub fn unpriced_calls(&self) -> u32 {
        self.tokens.iter().map(|row| row.unpriced_calls).sum()
    }
}
