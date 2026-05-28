//! # aura-fleet-quota
//!
//! Layer: fleet
//!
//! Concurrency and resource budgets across the fleet.
//!
//! ## Phase 7a — tracking only, no enforcement
//!
//! **Important**: in Phase 7a `QuotaPool::try_acquire` ALWAYS
//! succeeds. The pool records (a) the per-agent ticket and (b) a
//! monotonically-incrementing wall-clock counter of outstanding
//! tickets so observability + tests can verify the spawn pipeline
//! consulted the pool, but no cap is enforced today. Real
//! enforcement (concurrent-agent ceiling, depth-aware sub-quotas,
//! per-user token budgets) lands in Phase 7b alongside `BudgetTicket`
//! release semantics.
//!
//! The tracking-only design is deliberate: Phase 7a's compatibility
//! adapter must produce a byte-identical [`aura_core::SubagentResult`]
//! for the existing task tool, and the legacy path performed zero
//! quota work. Wiring a quota call here without enforcement keeps
//! the call ordering the Phase 7b plan requires (`acquire` BEFORE
//! lease BEFORE audit write) without changing observable behaviour.
//!
//! ## Invariants (per `.cursor/rules.md` §13)
//!
//! - Every successful `try_acquire` returns a unique
//!   [`BudgetTicket::ticket_id`] (UUID v4) — useful for correlating
//!   across spawn / dispatch / audit logs.
//! - The internal counter increments before the lock is released so
//!   concurrent acquires never observe duplicate ticket ids or out-
//!   of-order issuance.
//!
//! ## Assumptions
//!
//! - `aura-fleet-spawn` is the only producer of `try_acquire` calls
//!   in Phase 7a; Phase 7b extends the surface to dispatch + plugin
//!   activations.
//! - The tracking counter is intended for observability only —
//!   tests assert behaviour, not cap enforcement.
//!
//! ## Failure modes
//!
//! - [`QuotaError::Internal`] — reserved for future enforcement;
//!   no variant currently fires in Phase 7a.

#![forbid(unsafe_code)]
#![warn(clippy::all)]

use aura_core::AgentId;
use parking_lot::Mutex;
use thiserror::Error;
use tracing::debug;
use uuid::Uuid;

/// Requested per-spawn budget. Phase 7a treats every field as
/// advisory; Phase 7b uses these to enforce sub-quotas.
#[derive(Debug, Clone, Copy)]
pub struct QuotaRequest {
    /// Spawning agent (or root agent for top-level spawns).
    pub agent_id: AgentId,
    /// Requested max iteration ceiling. Phase 7a records only.
    pub max_iterations: u32,
    /// Requested concurrent-tool ceiling. Phase 7a records only.
    pub max_concurrent_tools: u32,
    /// Optional token budget. Phase 7a records only.
    pub token_budget: Option<u64>,
}

/// Receipt for a successful `try_acquire`. Phase 7a treats this as
/// an opaque correlator; Phase 7b will require `release` to free
/// the slot.
#[derive(Debug, Clone)]
pub struct BudgetTicket {
    /// Unique correlation id for this acquire.
    pub ticket_id: Uuid,
    /// Agent the ticket was issued to.
    pub agent_id: AgentId,
    /// Recorded max iterations (mirrors [`QuotaRequest::max_iterations`]).
    pub max_iterations: u32,
    /// Recorded concurrent-tool cap.
    pub max_concurrent_tools: u32,
    /// Recorded optional token budget.
    pub token_budget: Option<u64>,
}

/// Errors returned by [`QuotaPool`]. Phase 7a defines the taxonomy
/// for future use; no variant fires today because acquires always
/// succeed.
#[derive(Debug, Error)]
pub enum QuotaError {
    /// Reserved for future cap-exceeded enforcement (Phase 7b).
    #[error("quota exceeded for agent {agent_id}: {reason}")]
    Exceeded {
        /// Agent that hit the cap.
        agent_id: AgentId,
        /// Human-readable reason.
        reason: String,
    },
    /// Reserved for internal accounting bugs (counter overflow,
    /// poisoned lock); none currently fires.
    #[error("quota pool internal error: {0}")]
    Internal(String),
}

/// Concurrency / resource budget pool. Phase 7a impl is tracking-
/// only — see crate docs.
#[derive(Debug)]
pub struct QuotaPool {
    /// In-flight acquired tickets (Phase 7a never decrements;
    /// Phase 7b adds explicit `release`).
    inner: Mutex<Vec<BudgetTicket>>,
}

impl Default for QuotaPool {
    fn default() -> Self {
        Self::new()
    }
}

impl QuotaPool {
    /// Construct an empty pool.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Vec::new()),
        }
    }

    /// Tracking-only acquire — always succeeds and records the
    /// request.
    ///
    /// # Errors
    ///
    /// No variant of [`QuotaError`] currently fires; the `Result`
    /// shape is preserved for Phase 7b enforcement.
    pub fn try_acquire(&self, request: QuotaRequest) -> Result<BudgetTicket, QuotaError> {
        let ticket = BudgetTicket {
            ticket_id: Uuid::new_v4(),
            agent_id: request.agent_id,
            max_iterations: request.max_iterations,
            max_concurrent_tools: request.max_concurrent_tools,
            token_budget: request.token_budget,
        };
        let mut guard = self.inner.lock();
        guard.push(ticket.clone());
        debug!(
            ticket_id = %ticket.ticket_id,
            agent_id = %ticket.agent_id,
            max_iterations = ticket.max_iterations,
            max_concurrent_tools = ticket.max_concurrent_tools,
            token_budget = ?ticket.token_budget,
            outstanding = guard.len(),
            "quota pool: tracking-only acquire"
        );
        Ok(ticket)
    }

    /// Snapshot count of outstanding tickets — useful for tests and
    /// observability dashboards.
    #[must_use]
    pub fn outstanding(&self) -> usize {
        self.inner.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(agent_id: AgentId) -> QuotaRequest {
        QuotaRequest {
            agent_id,
            max_iterations: 50,
            max_concurrent_tools: 4,
            token_budget: Some(64_000),
        }
    }

    #[test]
    fn try_acquire_always_succeeds_in_phase_7a() {
        let pool = QuotaPool::new();
        let id = AgentId::generate();
        let ticket = pool.try_acquire(request(id)).expect("phase 7a always ok");
        assert_eq!(ticket.agent_id, id);
        assert_eq!(ticket.max_iterations, 50);
        assert_eq!(ticket.max_concurrent_tools, 4);
        assert_eq!(ticket.token_budget, Some(64_000));
        assert_eq!(pool.outstanding(), 1);
    }

    #[test]
    fn try_acquire_issues_distinct_ticket_ids() {
        let pool = QuotaPool::new();
        let id = AgentId::generate();
        let a = pool.try_acquire(request(id)).unwrap();
        let b = pool.try_acquire(request(id)).unwrap();
        assert_ne!(a.ticket_id, b.ticket_id);
        assert_eq!(pool.outstanding(), 2);
    }

    #[test]
    fn try_acquire_tracks_distinct_agents_independently() {
        let pool = QuotaPool::new();
        let a = AgentId::generate();
        let b = AgentId::generate();
        pool.try_acquire(request(a)).unwrap();
        pool.try_acquire(request(b)).unwrap();
        pool.try_acquire(request(a)).unwrap();
        assert_eq!(pool.outstanding(), 3);
    }
}
