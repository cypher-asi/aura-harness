//! Layer: gate
//!
//! Gate's attempt log: a local file of what each attempt at a prompt
//! left behind and of what later proved it right or wrong, kept so Gate's
//! verdicts, and any new way of reaching one, can be scored against what
//! happened.
//!
//! The file is `gate/attempts.jsonl` under the data directory, one JSON
//! object per line, appended to and never rewritten. It stays on this
//! machine: the data directory is not under version control. A line is
//! one of three kinds, told apart by its `kind`:
//!
//! - `attempt` ([`Attempt`]): one model's work on a prompt, written when
//!   its `Outcome` is recorded: what Gate made of it, and its
//!   [`Evidence`](aura_gate_evidence::Evidence)
//! - `truth` ([`Truth`]): something outside the attempt that says whether
//!   its work was right: a benchmark task's own check, or the user
//!   rewinding it, sending it again to a stronger model, or picking
//!   between runs
//! - `review` ([`Review`]): what Gate's judge made of an attempt that was
//!   about to stand (see `aura-gate-followup`), asked as the attempt ended
//!   or later by `aura gate replay`
//!
//! A truth or a review belongs to the attempt with the same `agent` and
//! `prompt_tx`. A turn Gate moves to a stronger model is a new attempt
//! under the prompt recorded for the move, so the pair names one attempt.
//!
//! [`evaluate`] joins them and reports how often Gate let failed work
//! stand, for each [`Flag`](aura_gate_evidence::Flag) how many of those
//! failures it would have caught against how many good attempts it would
//! have questioned, and how the reviews held up. [`tiers`] reads from the
//! same lines what each flag has earned: nothing, a review of the
//! attempts it is raised on, or sending them straight back to the model.
//!
//! - `line`: the three kinds of [`Line`], and the attempt each is about
//! - `file`: the [`AttemptLog`] on disk, appended to and read back
//! - `join`: each attempt once, with its truth and the reviews of it
//! - `score`: [`evaluate`] and [`evaluate_where`], and their tallies
//! - `tiers`: what each sign of failure has earned ([`tiers`])
//! - `report`: the [`Report`] as text, for a terminal
//! - `samples` (tests only): the lines and logs the unit tests share
#![deny(clippy::too_many_lines)]

mod file;
mod join;
mod line;
mod report;
#[cfg(test)]
mod samples;
mod score;
mod tiers;

pub use file::{read, AttemptLog, Read, FILE};
pub use join::{stood, Stood};
pub use line::{Attempt, CopyAt, Kind, Line, Review, Terms, Truth, TruthSource};
pub use score::{
    evaluate, evaluate_where, Filter, FlagScore, Report, ReviewScore, Scored, Verdicts,
};
pub use tiers::{below_strongest, tiers, SignRecord, Tier, Tiers};
