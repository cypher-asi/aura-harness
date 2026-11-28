//! One named secret in the OS keychain, for other crates.
//!
//! A public face on the vault's [`SecretSlot`]: the same keychain
//! service (`aura`), the same owner-only fallback file when no keychain
//! backend exists, and the same WARN when it falls back.

use crate::error::AuthError;
use crate::vault::SecretSlot;
use std::path::PathBuf;

/// A secret stored in the OS keychain under `aura/<user>`, with a
/// `0600` fallback file for machines without a keychain.
#[derive(Debug, Clone)]
pub struct KeychainSecret {
    slot: SecretSlot,
}

impl KeychainSecret {
    /// The secret kept as keychain entry `aura/<user>`, or in
    /// `fallback_file` when the keychain is unavailable.
    #[must_use]
    pub fn new(user: impl Into<String>, fallback_file: PathBuf) -> Self {
        Self {
            slot: SecretSlot::keyring(user, fallback_file),
        }
    }

    /// The stored secret; `None` when neither the keychain nor the
    /// fallback file holds it. On a machine with no keychain backend
    /// only the fallback file is consulted.
    ///
    /// # Errors
    /// [`AuthError::CredentialKeyring`] when the keychain failed some
    /// other way and the fallback file is empty, so whether a secret
    /// exists is unknown.
    pub fn read(&self) -> Result<Option<String>, AuthError> {
        self.slot.read_answered()
    }

    /// Store `secret`, replacing any kept before.
    ///
    /// # Errors
    /// Unexpected keychain failures, and fallback file write failures.
    pub fn save(&self, secret: &str) -> Result<(), AuthError> {
        self.slot.save(secret)
    }
}
