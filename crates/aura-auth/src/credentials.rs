//! OS-native credential storage with a file-based fallback.
//!
//! Persists authentication sessions in the platform credential store —
//! DPAPI on Windows, the Keychain on macOS, and the Secret Service
//! (libsecret / gnome-keyring / KWallet) on Linux — so the JWT is
//! protected by the user's OS login rather than written to disk in
//! plaintext.
//!
//! When the platform credential service is unavailable (headless Linux
//! without a running secret-service daemon, for example), we fall back
//! to `~/.aura/credentials.json` with mode 0600 on Unix. The fallback
//! path is logged as a warning so operators can see why their secrets
//! landed on disk.
//!
//! (Wave 5 / T5.)

use crate::error::AuthError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{debug, warn};

/// Keyring service identifier used for every credential entry.
const KEYRING_SERVICE: &str = "aura";
/// Keyring username/slot under the service.
const KEYRING_USER: &str = "credentials";

/// Persisted authentication session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSession {
    /// JWT access token for the aura-router proxy.
    pub access_token: String,
    /// zOS user ID.
    pub user_id: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Primary zID (e.g. `0://alice`).
    pub primary_zid: String,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
}

/// Credential store backed by the OS keyring with a file fallback.
pub struct CredentialStore;

impl CredentialStore {
    /// Save a session in the OS credential store.
    ///
    /// On a [`keyring::Error::NoStorageAccess`] (headless Linux, CI images
    /// without a secret-service daemon) we downgrade to the legacy 0600
    /// credentials file and emit a WARN log so the operator can see the
    /// fallback.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::CredentialKeyring`] on unexpected keyring
    /// failures, [`AuthError::NoHomeDir`] when the fallback path cannot be
    /// resolved, or [`AuthError::CredentialIo`] on filesystem failures.
    pub fn save(session: &StoredSession) -> Result<(), AuthError> {
        let json = serde_json::to_string(session)?;

        match keyring_entry().and_then(|e| e.set_password(&json)) {
            Ok(()) => {
                debug!("Credentials saved to OS keyring");
                Ok(())
            }
            Err(e) if is_no_storage_access(&e) => {
                warn!(
                    error = %e,
                    "OS keyring unavailable; falling back to ~/.aura/credentials.json (0600)"
                );
                save_to_file(&json)
            }
            Err(e) => Err(AuthError::CredentialKeyring(e.to_string())),
        }
    }

    /// Load the stored session, if any.
    ///
    /// Tries the OS keyring first; on `NoStorageAccess` OR any
    /// `NoEntry`-flavoured failure we probe the legacy file path so
    /// existing users are not logged out during the keyring rollout.
    pub fn load() -> Option<StoredSession> {
        match keyring_entry().and_then(|e| e.get_password()) {
            Ok(json) => parse_session(&json, "keyring"),
            Err(e) if is_no_entry(&e) => load_from_file(),
            Err(e) if is_no_storage_access(&e) => {
                warn!(error = %e, "OS keyring unavailable; reading credentials from file fallback");
                load_from_file()
            }
            Err(e) => {
                warn!(error = %e, "Failed to read from OS keyring; trying file fallback");
                load_from_file()
            }
        }
    }

    /// Convenience: load only the JWT access token.
    #[must_use]
    pub fn load_token() -> Option<String> {
        Self::load().map(|s| s.access_token)
    }

    /// Delete the credentials from both the keyring and the fallback file.
    ///
    /// Succeeds silently when neither backing store has an entry.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::CredentialKeyring`] only on unexpected keyring
    /// failures (storage access / generic errors are logged + ignored).
    pub fn clear() -> Result<(), AuthError> {
        match keyring_entry().and_then(|e| e.delete_credential()) {
            Ok(()) => debug!("Credentials cleared from OS keyring"),
            Err(e) if is_no_entry(&e) || is_no_storage_access(&e) => {
                debug!(error = %e, "Keyring clear: no entry or no storage");
            }
            Err(e) => warn!(error = %e, "Keyring clear failed; continuing with file fallback"),
        }

        let path = credentials_path()?;
        match std::fs::remove_file(&path) {
            Ok(()) => debug!(?path, "Credentials file cleared"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(AuthError::CredentialIo { path, source: e }),
        }
        Ok(())
    }
}

/// Build a keyring entry handle for the fixed (service, user) tuple.
fn keyring_entry() -> keyring::Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
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

fn parse_session(raw: &str, source: &str) -> Option<StoredSession> {
    match serde_json::from_str::<StoredSession>(raw) {
        Ok(session) => {
            debug!(%source, user_id = %session.user_id, "Loaded stored credentials");
            Some(session)
        }
        Err(e) => {
            warn!(%source, error = %e, "Stored credentials have invalid format");
            None
        }
    }
}

/// Resolve the credentials file path (`~/.aura/credentials.json`).
fn credentials_path() -> Result<PathBuf, AuthError> {
    dirs::home_dir()
        .map(|h| h.join(".aura").join("credentials.json"))
        .ok_or(AuthError::NoHomeDir)
}

/// Write the serialized session to the legacy 0600 file.
///
/// Mirrors the previous behavior exactly; kept as an inline helper so the
/// keyring path and fallback path share all of the directory-creation and
/// error-wrapping logic.
fn save_to_file(json: &str) -> Result<(), AuthError> {
    let path = credentials_path()?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AuthError::CredentialIo {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }

    #[cfg(unix)]
    {
        use std::fs::OpenOptions;
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| AuthError::CredentialIo {
                path: path.clone(),
                source: e,
            })?;
        file.write_all(json.as_bytes())
            .map_err(|e| AuthError::CredentialIo {
                path: path.clone(),
                source: e,
            })?;
    }

    #[cfg(not(unix))]
    {
        std::fs::write(&path, json).map_err(|e| AuthError::CredentialIo {
            path: path.clone(),
            source: e,
        })?;
    }

    debug!(?path, "Credentials saved to file fallback");
    Ok(())
}

fn load_from_file() -> Option<StoredSession> {
    let path = credentials_path().ok()?;
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            warn!(?path, error = %e, "Failed to read credentials file");
            return None;
        }
    };
    parse_session(&data, "file")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stored_session_round_trip() {
        let session = StoredSession {
            access_token: "tok_abc".to_string(),
            user_id: "user-1".to_string(),
            display_name: "Alice".to_string(),
            primary_zid: "0://alice".to_string(),
            created_at: Utc::now(),
        };

        let json = serde_json::to_string(&session).unwrap();
        let restored: StoredSession = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.access_token, session.access_token);
        assert_eq!(restored.user_id, session.user_id);
        assert_eq!(restored.display_name, session.display_name);
        assert_eq!(restored.primary_zid, session.primary_zid);
    }

    #[test]
    fn test_parse_session_malformed_returns_none() {
        assert!(parse_session("not json at all", "test").is_none());
    }

    #[test]
    fn test_parse_session_valid_returns_some() {
        let raw = r#"{
            "access_token":"tok",
            "user_id":"u",
            "display_name":"n",
            "primary_zid":"z",
            "created_at":"2025-01-01T00:00:00Z"
        }"#;
        let parsed = parse_session(raw, "test").unwrap();
        assert_eq!(parsed.access_token, "tok");
    }
}
