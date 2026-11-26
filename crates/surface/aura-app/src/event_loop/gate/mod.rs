//! Gate's runtime in the app: what the turns and subtasks it routes do
//! between their model calls, over [`aura_gate`]'s pure parts, and what
//! `/gate` shows of it.
//!
//! - [`followup`]: what an attempt does when it ends with no check of its own
//! - [`subtask`]: how a `task` spawn is routed, judged and recorded
//! - [`turn`]: Gate's part of a terminal turn, from its label to its outcome
//! - [`view`]: `/gate`, what Gate has learned and how it has routed

pub(crate) mod followup;
pub(super) mod subtask;
pub(super) mod turn;
pub(super) mod view;
