//! What `/compare` shows: the estimate before a comparison starts, the
//! per-arm (and, for a replayed session, per-prompt) view once it ends,
//! the list of past comparisons, and run diffs split per file for the
//! diff view.
//!
//! - `format`: how amounts, durations, token counts, headlines, arms and
//!   statuses read
//! - `estimate`: what the user is asked to confirm before a comparison
//!   starts
//! - `summary`: the view of one comparison
//! - `list`: one line per past comparison
//! - `patch`: a run's diff split per file

mod estimate;
mod format;
mod list;
mod patch;
mod summary;
#[cfg(test)]
mod tests;

pub(super) use estimate::EstimateView;
pub(super) use format::arm_label;
pub(crate) use format::{duration, headline, money, tokens};
pub(super) use list::list;
pub(super) use patch::split_patch;
pub(crate) use summary::render;
