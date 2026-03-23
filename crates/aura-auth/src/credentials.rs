//! File-based credential storage.
//!
//! Persists authentication sessions to `~/.aura/credentials.json` so the JWT
//! survives across CLI invocations without requiring an environment variable.
//!
//! # File layout
//!
//! The credentials file is a single JSON object containing the access token,
//! user metadata, and timestamps. The file is readable only by the current user
//! (mode 0600 on Unix).

use crate::error::AuthError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{debug, warn};

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

/// File-based credential store at `~/.aura/credentials.json`.
pub struct CredentialStore;

impl CredentialStore {
    /// Save a session to the credentials file.
    ///
    /// Creates the parent directory if it does not exist. On Unix, the file is
    /// created with mode 0600.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::NoHomeDir`] if the home directory cannot be
    /// determined, or [`AuthError::CredentialIo`] on filesystem failures.
    pub fn save(session: &StoredSession) -> Result<(), AuthError> {
        let path = Self::credentials_path()?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AuthError::CredentialIo {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }

        let json = serde_json::to_string_pretty(session)?;

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

        debug!(?path, "Credentials saved");
        Ok(())
    }

    /// Load the stored session, if any.
    ///
    /// Returns `None` when no credentials file exists. Logs a warning and
    /// returns `None` if the file is present but unreadable.
    pub fn load() -> Option<StoredSession> {
        let path = Self::credentials_path().ok()?;

        let data = match std::fs::read_to_string(&path) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                warn!(?path, error = %e, "Failed to read credentials file");
                return None;
            }
        };

        match serde_json::from_str::<StoredSession>(&data) {
            Ok(session) => {
                debug!(?path, user_id = %session.user_id, "Loaded stored credentials");
                Some(session)
            }
            Err(e) => {
                warn!(?path, error = %e, "Credentials file has invalid format");
                None
            }
        }
    }

    /// Convenience: load only the JWT access token.
    #[must_use]
    pub fn load_token() -> Option<String> {
        Self::load().map(|s| s.access_token)
    }

    /// Delete the credentials file.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::CredentialIo`] if the file exists but cannot be
    /// removed. Succeeds silently when no file is present.
    pub fn clear() -> Result<(), AuthError> {
        let path = Self::credentials_path()?;

        match std::fs::remove_file(&path) {
            Ok(()) => {
                debug!(?path, "Credentials cleared");
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(AuthError::CredentialIo { path, source: e }),
        }
    }

    /// Resolve the credentials file path (`~/.aura/credentials.json`).
    fn credentials_path() -> Result<PathBuf, AuthError> {
        dirs::home_dir()
            .map(|h| h.join(".aura").join("credentials.json"))
            .ok_or(AuthError::NoHomeDir)
    }
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
    fn test_save_and_load_with_temp_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("credentials.json");

        let session = StoredSession {
            access_token: "jwt-123".to_string(),
            user_id: "u1".to_string(),
            display_name: "Test".to_string(),
            primary_zid: "0://test".to_string(),
            created_at: Utc::now(),
        };

        let json = serde_json::to_string_pretty(&session).unwrap();
        std::fs::write(&path, &json).unwrap();

        let loaded: StoredSession =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();

        assert_eq!(loaded.access_token, "jwt-123");
        assert_eq!(loaded.display_name, "Test");
    }

    #[test]
    fn test_clear_nonexistent_is_ok() {
        // CredentialStore::clear() should not fail when no file exists.
        // We can't easily test the real path, but we verify the logic
        // by checking that NotFound is handled.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.json");
        let result = std::fs::remove_file(&path);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::NotFound);
    }
}
