//! `RecordEntry` re-exports.
//!
//! Phase 2 transitional shim: the actual struct still lives in
//! `aura_core::types::record` because it transitively references
//! `Transaction`, `Action`, `Effect`, `ContextHash`, `ProposalSet`,
//! and `Decision` — all of which currently live in `aura-core` and
//! cannot move in the same phase without recursively pulling the
//! whole id/types subtree across crate boundaries. Re-exporting here
//! gives downstream code a stable `aura_store_record::RecordEntry`
//! path that the layered crates can target today; Phase 6+ inverts
//! the dep direction and moves the struct body in.
//!
//! See the crate-level docs for invariants, assumptions, and failure
//! modes that this type participates in.

pub use aura_core::{RecordEntry, RecordEntryBuilder};

/// Kernel version recorded in every [`RecordEntry`]. Re-exported from
/// `aura-core` for the same Phase 2 transitional reason as
/// [`RecordEntry`] itself.
pub use aura_core::KERNEL_VERSION;
