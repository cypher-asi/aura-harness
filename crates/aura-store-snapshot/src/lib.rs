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
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors surfaced by [`SnapshotStore`] implementations.
#[derive(Debug, Error)]
pub enum SnapshotError {
    /// Storage backend failure (I/O, corruption, transient unavailability).
    #[error("snapshot backend error: {0}")]
    Backend(String),
    /// The hash can't name a stored object: it is empty or has characters
    /// other than ASCII letters, digits, `-` and `_`.
    #[error("invalid snapshot hash: {0:?}")]
    InvalidHash(String),
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

/// Snapshot store keeping each object in its own file under a directory,
/// named by its hash.
///
/// Objects are written through a temporary file and renamed into place,
/// so a crash never leaves a partial object under a valid name. Hashes are
/// used as file names, so only `[A-Za-z0-9_-]` is accepted.
#[derive(Clone, Debug)]
pub struct DirSnapshotStore {
    root: PathBuf,
}

impl DirSnapshotStore {
    /// A store keeping its objects under `root`, which is created on the
    /// first write.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The directory holding the objects.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn object_path(&self, hash: &str) -> Result<PathBuf, SnapshotError> {
        let valid = !hash.is_empty()
            && hash
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if valid {
            Ok(self.root.join(hash))
        } else {
            Err(SnapshotError::InvalidHash(hash.to_string()))
        }
    }
}

fn backend(context: &str, path: &Path, e: &std::io::Error) -> SnapshotError {
    SnapshotError::Backend(format!("{context} {}: {e}", path.display()))
}

impl SnapshotStore for DirSnapshotStore {
    fn put(&self, hash: &str, bytes: Bytes) -> Result<(), SnapshotError> {
        let path = self.object_path(hash)?;
        if path.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.root).map_err(|e| backend("create", &self.root, &e))?;
        let tmp = self.root.join(format!(".{hash}.tmp"));
        std::fs::write(&tmp, &bytes).map_err(|e| backend("write", &tmp, &e))?;
        std::fs::rename(&tmp, &path).map_err(|e| backend("rename", &path, &e))
    }

    fn get(&self, hash: &str) -> Result<Option<Bytes>, SnapshotError> {
        let path = self.object_path(hash)?;
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(Bytes::from(bytes))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(backend("read", &path, &e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_store_round_trips_and_ignores_repeat_puts() {
        let dir = tempfile::tempdir().unwrap();
        let store = DirSnapshotStore::new(dir.path().join("blobs"));
        assert!(store.get("abc").unwrap().is_none());
        store.put("abc", Bytes::from_static(b"first")).unwrap();
        store.put("abc", Bytes::from_static(b"second")).unwrap();
        assert_eq!(store.get("abc").unwrap().unwrap(), &b"first"[..]);
    }

    #[test]
    fn dir_store_rejects_hashes_that_are_not_plain_names() {
        let dir = tempfile::tempdir().unwrap();
        let store = DirSnapshotStore::new(dir.path());
        for hash in ["", "../x", "a/b", "a.b"] {
            assert!(matches!(
                store.put(hash, Bytes::new()),
                Err(SnapshotError::InvalidHash(_))
            ));
            assert!(matches!(
                store.get(hash),
                Err(SnapshotError::InvalidHash(_))
            ));
        }
    }

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
