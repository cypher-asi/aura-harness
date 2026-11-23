//! Claiming an agent's next sequence number.
//!
//! The store takes an agent's entries in order only, each at one past its
//! head. Every record path of a kernel claims its number here and keeps
//! the kernel's sequence lock until its entry is appended, so two records
//! of one kernel, such as `reason` calls answered at once, never claim
//! the same number.

use aura_core_types::AgentId;
use aura_store_db::Store;
use std::sync::{Mutex, MutexGuard, PoisonError};

#[cfg(test)]
mod tests;

/// A sequence number claimed for an agent's next entry. The kernel's
/// sequence lock is held until this drops: append the entry at
/// [`Claim::seq`] first.
pub(super) struct Claim<'a> {
    /// The number the entry is appended at
    pub seq: u64,
    _held: MutexGuard<'a, u64>,
}

/// Claims `agent_id`'s next sequence number, one past the store's head,
/// under `next`, the kernel's sequence lock, which afterwards holds the
/// number after the claimed one.
///
/// # Errors
/// Fails when the store's head cannot be read.
pub(super) fn claim<'a>(
    store: &dyn Store,
    agent_id: AgentId,
    next: &'a Mutex<u64>,
) -> Result<Claim<'a>, crate::KernelError> {
    let mut held = next.lock().unwrap_or_else(PoisonError::into_inner);
    let head = store
        .get_head_seq(agent_id)
        .map_err(|e| crate::KernelError::Store(format!("get_head_seq: {e}")))?;
    let seq = head + 1;
    *held = seq + 1;
    Ok(Claim { seq, _held: held })
}
