//! Layer: gate
//!
//! Gate while the app runs: the routing table every agent's turns share,
//! the attempt log as attempts end, and Gate's own calls to other models,
//! which go through an agent's kernel so they are recorded.
//!
//! - `table`: [`GateLive`], the table Gate routes by, with the latest
//!   decisions for `/gate` and the model each agent's work is on
//! - `rebuild` and `observer`: the table rebuilt from the record at
//!   startup, then kept current as every kernel's record observer
//! - `attempts`: the attempt log's lines as attempts end, and catching up
//!   on comparisons recorded before the log had them
//! - `ask`: the classifier's, the criteria writer's and the judge's calls
//!   through an agent's kernel, each within a time of its own
//! - `record`: a turn's `Outcome`s on the record, and the user's later
//!   verdicts on the turn (a retry on a stronger model, a rewind)
//! - `runtime`: the clock, and threads startup doesn't wait for
#![deny(clippy::too_many_lines)]

mod ask;
mod attempts;
mod observer;
mod rebuild;
mod record;
mod runtime;
mod table;

pub use ask::{
    ask_review, classify_prompt, cost_text, hold_to_criteria, judge_failed, judge_for,
    judge_unverified, reviewer_for, verdict_judge_for, worker_note_for, Judged,
};
pub use attempts::{copy_at, evidence, evidence_of, sent_text, RunOf, SavedSessions};
pub use record::{last_turn, note_retry, note_rewind, record, LastTurn};
pub use table::{GateLive, RecentRoute};
