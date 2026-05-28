//! # aura-store-snapshot
//!
//! Layer: store
//!
//! V1 ships a no-op stub. Real snapshot I/O (content-addressed
//! payload storage for `KernelMode::AuditedLite` full-payload
//! retrieval and Phase 6b replay) lands in a later activation phase.
//! The trait and types are defined here so consumers can compile
//! against the contract today.
//!
//! ## Invariants (per `.cursor/rules.md` §13)
//!
//! - [`SnapshotStore::put`] is **content-addressed**: writing the same
//!   `(hash, bytes)` twice is idempotent. The no-op stub trivially
//!   satisfies this by accepting every write.
//! - [`SnapshotStore::get`] returns the bytes most recently
//!   associated with the given hash, or `None` if absent. The no-op
//!   stub always returns `None`.
//! - Hashes are opaque strings (typically hex-encoded BLAKE3) to keep
//!   the trait independent of the concrete digest crate that wires
//!   in later.
//!
//! ## Failure modes
//!
//! - [`SnapshotError::Backend`] — storage backend failure (disk, S3,
//!   network filesystem). The no-op stub never produces an error.
//!
//! ## Assumptions
//!
//! - Callers always content-address: the `hash` is derived from
//!   `bytes` before the put. Implementations MAY verify the hash
//!   matches, but the stub does not.

#![forbid(unsafe_code)]
#![warn(clippy::all)]

use bytes::Bytes;
use thiserror::Error;

/// Errors surfaced by [`SnapshotStore`] implementations.
#[derive(Debug, Error)]
pub enum SnapshotError {
    /// Storage backend failure (I/O, corruption, transient unavailability).
    #[error("snapshot backend error: {0}")]
    Backend(String),
}

/// Content-addressed snapshot store. See the crate docs for
/// invariants, assumptions, and failure modes.
///
/// Requires `Debug` so trait objects are diagnosable from
/// `#[derive(Debug)]` containers (notably
/// `aura_agent_kernel::KernelConfig` in Phase 6b, which needs to
/// log its config under `tracing`).
pub trait SnapshotStore: std::fmt::Debug + Send + Sync {
    /// Store `bytes` under the content address `hash`. Idempotent: a
    /// second put for the same hash is a no-op.
    ///
    /// # Errors
    ///
    /// Returns [`SnapshotError::Backend`] if the storage backend
    /// reports a write failure.
    fn put(&self, hash: &str, bytes: Bytes) -> Result<(), SnapshotError>;

    /// Retrieve the bytes previously stored under `hash`, or `None`
    /// if the hash is unknown.
    ///
    /// # Errors
    ///
    /// Returns [`SnapshotError::Backend`] if the storage backend
    /// reports a read failure.
    fn get(&self, hash: &str) -> Result<Option<Bytes>, SnapshotError>;
}

/// No-op stub. Always succeeds for `put`; always returns `None` for `get`.
///
/// This is the V1 default: the kernel can compile against the
/// `SnapshotStore` contract today, and `KernelMode::AuditedLite`
/// payload retrieval falls back to a live-model replay flag rather
/// than crashing when the snapshot store is unavailable.
#[derive(Default, Clone, Debug)]
pub struct NoopSnapshotStore;

impl SnapshotStore for NoopSnapshotStore {
    fn put(&self, _hash: &str, _bytes: Bytes) -> Result<(), SnapshotError> {
        Ok(())
    }

    fn get(&self, _hash: &str) -> Result<Option<Bytes>, SnapshotError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_put_succeeds_and_get_returns_none() {
        let s = NoopSnapshotStore;
        s.put("abc", Bytes::from_static(b"x")).unwrap();
        assert!(s.get("abc").unwrap().is_none());
    }

    #[test]
    fn noop_put_is_idempotent() {
        let s = NoopSnapshotStore;
        s.put("hash-1", Bytes::from_static(b"first")).unwrap();
        s.put("hash-1", Bytes::from_static(b"second")).unwrap();
        // Stub does not retain bytes, so we cannot assert content; the
        // important property is that repeated puts do not fail.
        assert!(s.get("hash-1").unwrap().is_none());
    }
}
