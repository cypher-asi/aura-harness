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
//! - **At rest.** A vault slot's fallback file is sealed
//!   ([`super::sealed`]); a plain one from before is migrated on its
//!   first read. A [`crate::KeychainSecret`]'s fallback (the local
//!   store's own key) stays plain: sealing a key with another key in a
//!   file beside it would add nothing.
//! - **Windows ACL.** `share_mode(0)` blocks concurrent opens while
//!   the tmp file is live but does not tighten the DACL — the file
//!   still inherits the parent directory's ACL. Tracked as a TODO below.

use super::sealed::{staged_path, FileSeal, Opened};
use crate::error::AuthError;
use std::io;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Keyring service identifier used for every credential entry.
pub(crate) const KEYRING_SERVICE: &str = "aura";

/// Extra keyring reads after a failed one, before falling back to the file.
const KEYRING_READ_RETRIES: u32 = 3;
const KEYRING_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(100);

/// What reading a [`SecretSlot`] found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Read {
    Found(String),
    /// Neither the keyring nor the file holds it
    Absent,
    /// The keyring failed and the file holds nothing, so whether the
    /// secret is stored is unknown
    Unanswered,
}

/// A named secret: keyring entry `aura/<user>` plus a fallback file.
#[derive(Debug, Clone)]
pub(crate) struct SecretSlot {
    keyring_user: Option<String>,
    path: PathBuf,
    /// Seals the fallback file; `None` keeps it plain
    seal: Option<FileSeal>,
}

impl SecretSlot {
    /// Keyring-first slot.
    pub(crate) fn keyring(user: impl Into<String>, path: PathBuf) -> Self {
        Self {
            keyring_user: Some(user.into()),
            path,
            seal: None,
        }
    }

    /// File-only slot (tests and keyring-less vaults).
    pub(crate) fn file(path: PathBuf) -> Self {
        Self {
            keyring_user: None,
            path,
            seal: None,
        }
    }

    /// This slot with its fallback file sealed by `seal`.
    pub(crate) fn sealed(mut self, seal: FileSeal) -> Self {
        self.seal = Some(seal);
        self
    }

    /// Write `secret` to the fallback file: sealed when the slot seals.
    fn write_file(&self, secret: &str) -> Result<(), AuthError> {
        let Some(seal) = &self.seal else {
            return save_to_path(&self.path, secret);
        };
        let sealed = seal.seal(secret)?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AuthError::CredentialIo {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        atomic_write_private(&self.path, &sealed).map_err(|e| AuthError::CredentialIo {
            path: self.path.clone(),
            source: e,
        })
    }

    /// The fallback file's secret. A sealed file that can't be opened is
    /// an error; a plain one from before is returned and sealed in place
    /// ([`FileSeal::migrate`]), or left as it is when that fails.
    fn read_file(&self) -> Result<Option<String>, AuthError> {
        let Some(seal) = &self.seal else {
            return Ok(load_from_path(&self.path));
        };
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(AuthError::CredentialIo {
                    path: self.path.clone(),
                    source: e,
                })
            }
        };
        match seal.open(&self.path, &bytes)? {
            Opened::Sealed(secret) => Ok(Some(secret)),
            Opened::Plain(plain) => {
                match seal.migrate(&self.path, &plain, atomic_write_private) {
                    Ok(()) => debug!(path = %self.path.display(), "Credentials file sealed"),
                    Err(e) => warn!(
                        path = %self.path.display(),
                        error = %e,
                        "Could not seal the plain credentials file; it is kept, and sealing is tried again next time"
                    ),
                }
                Ok(Some(plain.to_string()))
            }
        }
    }

    /// Store `secret`, downgrading to the 0600 file when no keyring
    /// backend exists.
    pub(crate) fn save(&self, secret: &str) -> Result<(), AuthError> {
        let Some(user) = &self.keyring_user else {
            return self.write_file(secret);
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
                     This is a DEGRADED MODE — the secret is sealed with a key file beside it. \
                     Install a secret-service daemon (gnome-keyring / KWallet) to re-enable keyring storage."
                );
                self.write_file(secret)
            }
            Err(e) => Err(AuthError::CredentialKeyring(e.to_string())),
        }
    }

    /// Read the secret, probing the file when the keyring has no entry
    /// so users are not logged out by a keyring rollout.
    pub(crate) fn read(&self) -> Read {
        let from_file = |unanswered: bool| match self.read_file() {
            Ok(Some(secret)) => Read::Found(secret),
            Ok(None) if unanswered => Read::Unanswered,
            Ok(None) => Read::Absent,
            Err(e) => {
                warn!(error = %e, "Could not read the credentials file");
                Read::Unanswered
            }
        };
        let Some(user) = &self.keyring_user else {
            return from_file(false);
        };
        // Another process replacing the entry (a token refresh) can leave
        // it briefly missing, and Secret Service calls fail transiently.
        let mut read = keyring_entry(user).and_then(|e| e.get_password());
        for _ in 0..KEYRING_READ_RETRIES {
            match &read {
                Err(e) if !is_no_storage_access(e) && !self.path.exists() => {
                    std::thread::sleep(KEYRING_RETRY_DELAY);
                    read = keyring_entry(user).and_then(|e| e.get_password());
                }
                _ => break,
            }
        }
        match read {
            Ok(secret) => Read::Found(secret),
            Err(e) if is_no_entry(&e) => from_file(false),
            Err(e) if is_no_storage_access(&e) => {
                warn!(error = %e, "OS keyring unavailable; reading credentials from file fallback");
                from_file(true)
            }
            Err(e) => {
                warn!(error = %e, "Failed to read from OS keyring; trying file fallback");
                from_file(true)
            }
        }
    }

    /// Like [`Self::read`], but a machine without a keyring backend
    /// counts as an answer: the fallback file is then the only place the
    /// secret can be, so no file means no secret. Other keyring failures
    /// are errors unless the file holds the secret.
    pub(crate) fn read_answered(&self) -> Result<Option<String>, AuthError> {
        let Some(user) = &self.keyring_user else {
            return self.read_file();
        };
        let mut read = keyring_entry(user).and_then(|e| e.get_password());
        for _ in 0..KEYRING_READ_RETRIES {
            match &read {
                Err(e) if !is_no_storage_access(e) && !is_no_entry(e) => {
                    std::thread::sleep(KEYRING_RETRY_DELAY);
                    read = keyring_entry(user).and_then(|e| e.get_password());
                }
                _ => break,
            }
        }
        match read {
            Ok(secret) => Ok(Some(secret)),
            Err(e) if is_no_entry(&e) => self.read_file(),
            Err(e) if is_no_storage_access(&e) => {
                debug!(error = %e, "OS keyring unavailable; reading the fallback file");
                self.read_file()
            }
            Err(e) => self
                .read_file()?
                .map(Some)
                .ok_or_else(|| AuthError::CredentialKeyring(e.to_string())),
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
        // Best-effort: nuke any leftover tmp or staged file from a crash.
        let _ = std::fs::remove_file(tmp_path_for(&self.path));
        let _ = std::fs::remove_file(staged_path(&self.path));
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
mod tests;
