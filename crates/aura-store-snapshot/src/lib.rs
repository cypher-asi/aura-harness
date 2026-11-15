//! # aura-store-snapshot
//!
//! Layer: store
//!
//! Content-addressed payload storage. [`FsSnapshotStore`] keeps
//! zstd-compressed blobs on disk; it backs run-trace observations (see
//! `aura_core_types::trace`) and the full payloads behind
//! `KernelMode::AuditedLite` summaries, which kernel replay resolves.
//! [`NoopSnapshotStore`] is the kernel default when no store is wired.
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

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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

/// Filesystem-backed content-addressed store.
///
/// Blobs live at `<root>/<first two hex chars>/<full hash>`, zstd-
/// compressed; the address is the BLAKE3 digest of the uncompressed
/// bytes. Writes go to a temporary sibling and are renamed into place,
/// so a crash never leaves a truncated blob under a valid address. `put`
/// verifies that `hash` is the BLAKE3 digest of `bytes`, which keeps the
/// store usable as a replay source. Blobs written uncompressed by older
/// versions still read back unchanged.
#[derive(Debug)]
pub struct FsSnapshotStore {
    root: PathBuf,
    known: Mutex<HashSet<String>>,
}

impl FsSnapshotStore {
    /// Open (and create if needed) a store rooted at `root`.
    ///
    /// # Errors
    ///
    /// Returns [`SnapshotError::Backend`] if the directory cannot be
    /// created.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, SnapshotError> {
        let root = root.into();
        std::fs::create_dir_all(&root)
            .map_err(|e| SnapshotError::Backend(format!("create {}: {e}", root.display())))?;
        Ok(Self {
            root,
            known: Mutex::new(HashSet::new()),
        })
    }

    /// Root directory of the store.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Hash `bytes`, store them, and return the address.
    ///
    /// # Errors
    ///
    /// Returns [`SnapshotError::Backend`] on write failure.
    pub fn put_bytes(&self, bytes: &[u8]) -> Result<String, SnapshotError> {
        let hash = blake3::hash(bytes).to_hex().to_string();
        self.put(&hash, Bytes::copy_from_slice(bytes))?;
        Ok(hash)
    }

    fn path_for(&self, hash: &str) -> Result<PathBuf, SnapshotError> {
        if hash.len() < 3 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(SnapshotError::Backend(format!(
                "invalid blob address {hash:?}"
            )));
        }
        Ok(self.root.join(&hash[..2]).join(hash))
    }

    fn is_known(&self, hash: &str) -> bool {
        self.known
            .lock()
            .map(|set| set.contains(hash))
            .unwrap_or(false)
    }

    fn mark_known(&self, hash: &str) {
        if let Ok(mut set) = self.known.lock() {
            set.insert(hash.to_string());
        }
    }
}

impl SnapshotStore for FsSnapshotStore {
    fn put(&self, hash: &str, bytes: Bytes) -> Result<(), SnapshotError> {
        if self.is_known(hash) {
            return Ok(());
        }
        let actual = blake3::hash(&bytes).to_hex();
        if actual.as_str() != hash {
            return Err(SnapshotError::Backend(format!(
                "blob address {hash} does not match content hash {actual}"
            )));
        }
        let path = self.path_for(hash)?;
        if !path.exists() {
            let dir = path.parent().unwrap_or(&self.root);
            std::fs::create_dir_all(dir)
                .map_err(|e| SnapshotError::Backend(format!("create {}: {e}", dir.display())))?;
            let tmp = dir.join(format!(".{hash}.{}.tmp", std::process::id()));
            let stored = zstd::bulk::compress(&bytes, ZSTD_LEVEL)
                .map_err(|e| SnapshotError::Backend(format!("compress {hash}: {e}")))?;
            std::fs::write(&tmp, &stored)
                .map_err(|e| SnapshotError::Backend(format!("write {}: {e}", tmp.display())))?;
            std::fs::rename(&tmp, &path)
                .map_err(|e| SnapshotError::Backend(format!("rename {}: {e}", path.display())))?;
        }
        self.mark_known(hash);
        Ok(())
    }

    fn get(&self, hash: &str) -> Result<Option<Bytes>, SnapshotError> {
        let path = self.path_for(hash)?;
        match std::fs::read(&path) {
            Ok(stored) => Ok(Some(Bytes::from(decode(hash, stored)))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(SnapshotError::Backend(format!(
                "read {}: {e}",
                path.display()
            ))),
        }
    }
}

const ZSTD_LEVEL: i32 = 3;
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// Decompress a stored blob, or return it as-is when it was stored raw.
fn decode(hash: &str, stored: Vec<u8>) -> Vec<u8> {
    if !stored.starts_with(&ZSTD_MAGIC) {
        return stored;
    }
    match zstd::stream::decode_all(stored.as_slice()) {
        Ok(raw) if blake3::hash(&raw).to_hex().as_str() == hash => raw,
        _ => stored,
    }
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

    #[test]
    fn fs_store_compresses_and_reads_raw_blobs() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsSnapshotStore::open(dir.path()).unwrap();
        let text = "fn main() {}\n".repeat(400);
        let id = store.put_bytes(text.as_bytes()).unwrap();
        let on_disk = std::fs::read(store.path_for(&id).unwrap()).unwrap();
        assert!(on_disk.starts_with(&ZSTD_MAGIC));
        assert!(on_disk.len() < text.len() / 10);
        assert_eq!(store.get(&id).unwrap().unwrap().as_ref(), text.as_bytes());

        let raw = b"written by an older version".to_vec();
        let raw_id = blake3::hash(&raw).to_hex().to_string();
        let path = store.path_for(&raw_id).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &raw).unwrap();
        assert_eq!(
            store.get(&raw_id).unwrap().unwrap().as_ref(),
            raw.as_slice()
        );
    }

    #[test]
    fn fs_store_roundtrips_and_dedups() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsSnapshotStore::open(dir.path()).unwrap();
        let id = store.put_bytes(b"payload").unwrap();
        assert_eq!(id, store.put_bytes(b"payload").unwrap());
        assert_eq!(store.get(&id).unwrap().unwrap().as_ref(), b"payload");

        let reopened = FsSnapshotStore::open(dir.path()).unwrap();
        assert_eq!(reopened.get(&id).unwrap().unwrap().as_ref(), b"payload");
    }

    #[test]
    fn fs_store_rejects_mismatched_address() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsSnapshotStore::open(dir.path()).unwrap();
        let wrong = blake3::hash(b"other").to_hex().to_string();
        assert!(store.put(&wrong, Bytes::from_static(b"payload")).is_err());
        assert!(store.get(&wrong).unwrap().is_none());
    }

    #[test]
    fn fs_store_rejects_path_like_addresses() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsSnapshotStore::open(dir.path()).unwrap();
        assert!(store.get("../etc/passwd").is_err());
    }
}
