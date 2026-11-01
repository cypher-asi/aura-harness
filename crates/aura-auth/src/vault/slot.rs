//! One secret in the OS credential store, with a file-based fallback.
//!
//! Secrets live in the platform credential store — DPAPI on Windows,
//! the Keychain on macOS, and the Secret Service (libsecret /
//! gnome-keyring / KWallet) on Linux — so tokens are protected by the
//! user's OS login rather than written to disk in plaintext.
//!
//! When the platform credential service is unavailable (headless Linux
//! without a running secret-service daemon, for example), the slot
//! falls back to its file path. The write is **atomic** (tmp-file +
//! rename) and the file inherits restrictive permissions — `0600` on
//! Unix; on Windows the tmp file is opened with `share_mode(0)` so
//! concurrent opens are blocked for the duration of the write. The
//! fallback path is logged at WARN so operators can see why secrets
//! landed on disk. (Wave 5 / T5, hardened in security Phase 7.)
//!
//! ## Security notes
//!
//! - **Atomicity.** A crash mid-write cannot leave a truncated JSON
//!   that looks like a corrupted secret: the final file is either the
//!   previous version or the new one. See [`atomic_write_private`].
//! - **Plaintext at rest.** The fallback file is still plaintext. Full
//!   encryption-at-rest (DPAPI / `age`) is a follow-up.
//! - **Windows ACL.** `share_mode(0)` blocks concurrent opens while
//!   the tmp file is live but does not tighten the DACL — the file
//!   still inherits the parent directory's ACL. Tracked as a TODO below.

use crate::error::AuthError;
use std::io;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Keyring service identifier used for every credential entry.
pub(crate) const KEYRING_SERVICE: &str = "aura";

/// A named secret: keyring entry `aura/<user>` plus a fallback file.
#[derive(Debug, Clone)]
pub(crate) struct SecretSlot {
    keyring_user: Option<String>,
    path: PathBuf,
}

impl SecretSlot {
    /// Keyring-first slot.
    pub(crate) fn keyring(user: impl Into<String>, path: PathBuf) -> Self {
        Self {
            keyring_user: Some(user.into()),
            path,
        }
    }

    /// File-only slot (tests and keyring-less vaults).
    pub(crate) fn file(path: PathBuf) -> Self {
        Self {
            keyring_user: None,
            path,
        }
    }

    /// Store `secret`, downgrading to the 0600 file when no keyring
    /// backend exists.
    pub(crate) fn save(&self, secret: &str) -> Result<(), AuthError> {
        let Some(user) = &self.keyring_user else {
            return save_to_path(&self.path, secret);
        };
        match keyring_entry(user).and_then(|e| e.set_password(secret)) {
            Ok(()) => {
                debug!(slot = %user, "Secret saved to OS keyring");
                // Drop any stale plaintext copy from an earlier fallback.
                let _ = std::fs::remove_file(&self.path);
                Ok(())
            }
            Err(e) if is_no_storage_access(&e) => {
                warn!(
                    error = %e,
                    path = %self.path.display(),
                    "OS keyring unavailable; falling back to on-disk credentials file. \
                     This is a DEGRADED MODE — the secret is written in plaintext. \
                     Install a secret-service daemon (gnome-keyring / KWallet) to re-enable keyring storage."
                );
                save_to_path(&self.path, secret)
            }
            Err(e) => Err(AuthError::CredentialKeyring(e.to_string())),
        }
    }

    /// Load the secret, probing the file when the keyring has no entry
    /// so users are not logged out by a keyring rollout.
    pub(crate) fn load(&self) -> Option<String> {
        let Some(user) = &self.keyring_user else {
            return load_from_path(&self.path);
        };
        match keyring_entry(user).and_then(|e| e.get_password()) {
            Ok(secret) => Some(secret),
            Err(e) if is_no_entry(&e) => load_from_path(&self.path),
            Err(e) if is_no_storage_access(&e) => {
                warn!(error = %e, "OS keyring unavailable; reading credentials from file fallback");
                load_from_path(&self.path)
            }
            Err(e) => {
                warn!(error = %e, "Failed to read from OS keyring; trying file fallback");
                load_from_path(&self.path)
            }
        }
    }

    /// Delete the secret from both backends. Succeeds when absent.
    pub(crate) fn clear(&self) -> Result<(), AuthError> {
        if let Some(user) = &self.keyring_user {
            match keyring_entry(user).and_then(|e| e.delete_credential()) {
                Ok(()) => debug!(slot = %user, "Secret cleared from OS keyring"),
                Err(e) if is_no_entry(&e) || is_no_storage_access(&e) => {
                    debug!(error = %e, "Keyring clear: no entry or no storage");
                }
                Err(e) => warn!(error = %e, "Keyring clear failed; continuing with file fallback"),
            }
        }
        match std::fs::remove_file(&self.path) {
            Ok(()) => debug!(path = ?self.path, "Credentials file cleared"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(AuthError::CredentialIo {
                    path: self.path.clone(),
                    source: e,
                })
            }
        }
        // Best-effort: nuke any leftover tmp file from a crashed write.
        let _ = std::fs::remove_file(tmp_path_for(&self.path));
        Ok(())
    }
}

fn keyring_entry(user: &str) -> keyring::Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, user)
}

/// Detect "no storage backend is available" errors — these are the ones
/// that should trigger the file fallback (headless Linux, locked-down
/// sandboxes, etc.). Other keyring errors are surfaced to the caller.
fn is_no_storage_access(e: &keyring::Error) -> bool {
    matches!(e, keyring::Error::NoStorageAccess(_))
}

/// Detect "no such entry" errors so `load`/`clear` can fall through to
/// the file backend rather than reporting a hard error on fresh installs.
fn is_no_entry(e: &keyring::Error) -> bool {
    matches!(e, keyring::Error::NoEntry)
}

/// Compute the sibling tmp-file path used by [`atomic_write_private`].
///
/// We keep the tmp file next to the final path (same directory, same
/// filesystem) so `rename` is a true atomic swap on both POSIX and NTFS.
fn tmp_path_for(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    path.with_file_name(name)
}

/// Save `contents` to `path` via an atomic tmp+rename dance.
pub(crate) fn save_to_path(path: &Path, contents: &str) -> Result<(), AuthError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AuthError::CredentialIo {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }

    atomic_write_private(path, contents.as_bytes()).map_err(|e| AuthError::CredentialIo {
        path: path.to_path_buf(),
        source: e,
    })?;

    debug!(?path, "Credentials saved to file (atomic)");
    Ok(())
}

pub(crate) fn load_from_path(path: &Path) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(d) => Some(d),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            warn!(?path, error = %e, "Failed to read credentials file");
            None
        }
    }
}

/// Atomically write `bytes` to `path` with restrictive permissions.
///
/// Writes to `<path>.tmp` first (creating it with platform-specific
/// private-file flags), fsyncs the data, then `rename`s the tmp file
/// over the final path. Both POSIX and NTFS guarantee `rename` within
/// a single directory is atomic: a concurrent reader sees either the
/// pre-write file or the post-write file, never a truncated prefix.
///
/// On Unix the tmp file is created `0o600` so even mid-write there is
/// no window during which group/other can read the JWT.
///
/// On Windows the tmp file is opened with `share_mode(0)` which blocks
/// any concurrent `CreateFileW` for the duration of the write, giving
/// the same "no half-written reads" property. Tightening the DACL down
/// to the owning SID requires the `windows` crate (not a current
/// workspace dep) and is tracked as a follow-up — see module docs.
fn atomic_write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;

    let tmp = tmp_path_for(path);
    // Clear any stale tmp from a previous crashed write so the
    // OpenOptions flags below apply to a fresh inode. `NotFound` is fine.
    match std::fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    {
        let mut file = open_private_for_write(&tmp)?;
        file.write_all(bytes)?;
        file.flush()?;
        // Fsync so the rename below can't expose a zero-length file
        // after a power loss on filesystems that reorder metadata.
        file.sync_all()?;
    }

    // rename is atomic on the same filesystem on both POSIX and NTFS
    // (Windows MoveFileExW performs a transacted replace for same-volume
    // targets). If rename fails, the previous file is untouched.
    if let Err(e) = std::fs::rename(&tmp, path) {
        // Best-effort cleanup so we don't leave a dangling tmp file.
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

#[cfg(unix)]
fn open_private_for_write(path: &Path) -> io::Result<std::fs::File> {
    use std::fs::OpenOptions;
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

#[cfg(windows)]
fn open_private_for_write(path: &Path) -> io::Result<std::fs::File> {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    // share_mode(0) = FILE_SHARE_NONE: no other handle may open this
    // file while ours is live. This doesn't tighten the DACL (TODO:
    // follow-up w/ `windows` crate to set an owner-only DACL) but it
    // does ensure no concurrent read/write race can expose a truncated
    // JWT during the brief window before rename.
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .share_mode(0)
        .open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_private_for_write(path: &Path) -> io::Result<std::fs::File> {
    // Unknown target: fall back to plain create+truncate. The outer
    // atomic_write_private still gives us tmp+rename semantics.
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_tmp_path_for_appends_suffix() {
        let p = Path::new("/a/b/credentials.json");
        assert_eq!(tmp_path_for(p), Path::new("/a/b/credentials.json.tmp"));
    }

    #[test]
    fn test_atomic_write_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("creds.json");
        atomic_write_private(&path, b"hello").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
    }

    #[test]
    fn test_atomic_write_overwrites_existing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("creds.json");
        std::fs::write(&path, b"old contents that are longer than new").unwrap();
        atomic_write_private(&path, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    }

    #[test]
    fn test_atomic_write_cleans_up_tmp_on_success() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("creds.json");
        atomic_write_private(&path, b"payload").unwrap();
        let tmp = tmp_path_for(&path);
        assert!(
            !tmp.exists(),
            "tmp file should be renamed away on success, but {tmp:?} still exists"
        );
    }

    #[test]
    fn test_atomic_write_clobbers_stale_tmp() {
        // A previous crashed write can leave <path>.tmp behind.
        // atomic_write_private must clobber it, not fail.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("creds.json");
        let tmp = tmp_path_for(&path);
        std::fs::write(&tmp, b"stale leftover from prior crash").unwrap();
        atomic_write_private(&path, b"fresh").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fresh");
        assert!(!tmp.exists());
    }

    #[test]
    fn test_save_and_load_from_path_round_trip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("credentials.json");
        save_to_path(&path, "{\"a\":1}").unwrap();
        assert_eq!(load_from_path(&path).as_deref(), Some("{\"a\":1}"));
    }

    #[test]
    fn test_load_from_path_missing_returns_none() {
        let dir = TempDir::new().unwrap();
        assert!(load_from_path(&dir.path().join("nope.json")).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn test_unix_perms_on_final_file_are_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("creds.json");
        atomic_write_private(&path, b"x").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "final file should be 0600, got {mode:o}");
    }

    #[cfg(unix)]
    #[test]
    fn test_unix_perms_on_tmp_file_are_0600() {
        // We can't observe the tmp file after rename, so we stub in a
        // pre-existing tmp and check its mode right after the
        // inner open_private_for_write call by invoking it directly.
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let tmp = dir.path().join("creds.json.tmp");
        {
            let _f = open_private_for_write(&tmp).unwrap();
        }
        let mode = std::fs::metadata(&tmp).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "tmp file should be 0600, got {mode:o}");
    }
}
