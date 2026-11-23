//! Layer: gate
//!
//! Where Gate routes work: the candidates the user's sign-ins allow, the
//! model a turn or subtask goes to, what Gate has learned of each model,
//! and how another way of routing would have done on the recorded work.
//!
//! - `candidate`: the models Gate may route to ([`candidates`], [`current`]),
//!   and those an arm of `/compare` may be pinned to ([`runnable`])
//! - `work`: a piece of work Gate routes ([`Work`])
//! - `decision`: where Gate sends a piece of work, and why ([`Decision`])
//! - `random`: the draws exploring takes ([`Random`], [`SplitMix64`])
//! - `cost`: what work is expected to cost on each model ([`by_expected_cost`])
//! - `judged`: the outcomes a model is judged by on a piece of work
//! - `start`: the model work starts on before a model is proven on it
//! - `effort`: how hard the model reasons on the work it is sent ([`effort_on`])
//! - `pick`: the model work goes to ([`pick()`], [`fixed_pick`], [`below_lead`])
//! - `escalate`: the model work goes on with after a failed attempt ([`escalate()`])
//! - [`stats`]: what Gate has learned per level, category and model
//! - [`replay`]: how another way of routing would have done on the record
//! - `fixtures`: the candidates and helpers the unit tests share
//!
//! Gate never calls a model itself: it chooses which model a turn runs
//! on, and the calls go through the agent's kernel as usual.

#![deny(clippy::too_many_lines)]

mod candidate;
mod cost;
mod decision;
mod effort;
mod escalate;
#[cfg(test)]
mod fixtures;
mod judged;
mod pick;
mod random;
pub mod replay;
mod start;
pub mod stats;
mod work;

pub use candidate::{candidates, current, runnable, Candidate};
pub use cost::by_expected_cost;
pub use decision::Decision;
pub use effort::effort_on;
pub use escalate::{escalate, has_stronger_model};
pub use pick::{below_lead, fixed_pick, pick};
pub use random::{Random, SplitMix64};
pub use work::Work;
