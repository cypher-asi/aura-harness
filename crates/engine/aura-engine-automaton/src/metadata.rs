//! Metadata-only declarations carried on [`crate::AutomatonInfo`].
//!
//! Phase 7 renamed `schedule.rs` → `metadata.rs` because the
//! [`Schedule`] enum looked operational (interval / cron variants
//! implying back-off semantics) but the [`crate::runtime`] spin
//! loop never actually enforced any of it — the runtime ticks
//! continuously regardless of variant. Keeping the type in a file
//! named `schedule.rs` was the "looks operational but isn't" trap
//! the plan called out; moving it under `metadata` makes the
//! advisory nature explicit at the import site
//! (`use crate::metadata::Schedule;`).
//!
//! The variants are still serialised onto [`crate::AutomatonInfo`]
//! so external tooling that introspects automaton instances can
//! tell what schedule a hand-rolled automaton *intended* to run
//! under (useful for operator dashboards / spec docs), but the
//! crate makes no runtime claim about that intent. If you need
//! real interval / event / on-demand semantics, plumb a sleep
//! through [`crate::runtime::AutomatonRuntime::run_automaton`]
//! explicitly.
//!
//! The original public surface (`Schedule`, `Schedule::is_continuous`,
//! `Schedule::is_on_demand`) is preserved verbatim so no caller has
//! to learn a new type name.

use serde::{Deserialize, Serialize};

/// Declarative metadata recorded on [`crate::AutomatonInfo`].
///
/// **Advisory only — the runtime does not enforce any variant.** The
/// spin loop in [`crate::runtime::AutomatonRuntime`] ticks
/// continuously regardless of the value carried here; the field
/// exists to label hand-rolled automata for operator tooling and
/// external schedulers. If you need real interval back-off or
/// event gating, build it inside the [`crate::runtime::Automaton::tick`]
/// implementation directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Schedule {
    Continuous,
    Interval { seconds: u64 },
    Cron { expression: String },
    OnDemand,
    EventDriven { event_filter: String },
}

impl Schedule {
    pub fn is_continuous(&self) -> bool {
        matches!(self, Self::Continuous)
    }

    pub fn is_on_demand(&self) -> bool {
        matches!(self, Self::OnDemand)
    }
}
