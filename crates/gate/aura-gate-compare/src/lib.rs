//! Layer: gate
//!
//! `/compare` and the BENCH tab: running the same work on two arms, each
//! Gate, a pinned model or a model per difficulty, side by side, and
//! reading the result.
//!
//! This crate holds the parts that need no model, kernel or UI:
//!
//! - `args`: reading `/compare`'s arguments
//! - `arms`: what each arm runs on ([`DifficultyRoutes`] for an arm with
//!   a model per difficulty), and the model Gate is compared with
//! - `estimate`: list prices, and the cost estimate shown before anything runs
//! - `schedule`: the order runs start in, and when the budget stops them
//! - `tokens`: the tokens and cost per model of a run
//! - `plan_shares`: the share of each plan window each run used
//! - `summary`: per-arm medians, pass counts and judge tallies, per prompt and task
//! - `pairs`: which runs the judge compares, in what order, and what it is told
//! - `source`: the prompts, title and file paths of a replayed session
//!
//! Running the arms, the check command and the judge live with the app.

#![deny(clippy::too_many_lines)]

mod args;
mod arms;
mod estimate;
mod pairs;
mod plan_shares;
mod schedule;
mod source;
mod summary;
mod tokens;

pub use args::{parse, CompareCommand, CompareRequest, SessionPick};
pub use arms::{default_against, resolve_arms, resolve_classifier, DifficultyRoutes, NO_DELEGATE};
pub use estimate::{estimate, list_price, Estimate};
pub use pairs::{judge_pairs, judge_task, seed, EndedRun};
pub use plan_shares::{plan_shares, PlanReading};
pub use schedule::{run_name, Schedule, DEFAULT_CONCURRENCY};
pub use source::{prompts, relative, title};
pub use summary::{
    judge_cost_usd, judge_unpriced_calls, summarize, task_cells, too_close, turn_cells, ArmSummary,
    TaskCell, TurnCell,
};
pub use tokens::{tally, ModelCall};
