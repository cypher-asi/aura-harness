//! What Gate has learned: per level, category and model, how much work
//! ended and what it cost, how much of it has a known result, and how
//! much of that succeeded.
//!
//! [`GateStats`] is built by feeding it record entries
//! ([`GateStats::observe`]): the log at startup, then every entry the
//! kernels commit. Each prompt counts once, with its latest outcome that
//! has a known result. An outcome that replaces an earlier one for the
//! same prompt takes its place in the table instead of adding to it, and
//! one that takes the earlier ones back (the judge found the failure was
//! not the model's doing, see [`aura_core_types::OutcomeSignal::voids`])
//! leaves the prompt out of the table. Work the user cancelled still
//! counts for what it cost: a model's cost is what all of its work cost.
//!
//! Entries may arrive in any order and more than once (a startup scan
//! can overlap the live feed): which outcome counts depends only on the
//! record sequence numbers, and seeing an entry again changes nothing.
//! A prompt's outcomes are in the same record as the prompt, so their
//! sequence numbers compare. Work that went on to a stronger model
//! after its first attempt failed (route reason `escalate`) is not
//! counted: it only ever sees work that already failed once, and the
//! first attempt's outcome already says how the route did. A prompt is
//! known by its agent and its transaction hash: the hash covers only the
//! payload, so two agents given the same prompt (the repeats of a
//! `/compare`) record the same hash for different work.

use std::collections::HashMap;

use aura_core_types::{AgentId, Hash, RecordEntry};

#[cfg(any(test, feature = "test-support"))]
mod learn;
mod observe;
mod query;
mod row;
mod table;

use observe::PromptState;
pub use row::Row;
pub use table::RowKey;
use table::Table;

/// Gate's table, and the per-prompt outcomes it is built from.
#[derive(Debug, Clone, Default)]
pub struct GateStats {
    /// What each model has done, by level and category, then by model.
    table: Table,
    /// What is known of each prompt, by its agent and transaction hash.
    prompts: HashMap<(AgentId, Hash), PromptState>,
}

impl GateStats {
    /// The table built from `entries`.
    #[must_use]
    pub fn from_entries<'a>(entries: impl IntoIterator<Item = &'a RecordEntry>) -> Self {
        let mut stats = Self::default();
        for entry in entries {
            stats.observe(entry);
        }
        stats
    }
}

#[cfg(test)]
mod tests;
