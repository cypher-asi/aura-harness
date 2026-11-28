//! Where the local store's key is kept.
//!
//! * **OS keychain** (default): entry `aura/store-key:<store id>`, where
//!   the store id is a random UUID kept in `<data_dir>/.aura-store-id`,
//!   so each data directory has its own key and moving the directory
//!   keeps it. When the machine has no keychain backend, `aura-auth`
//!   falls back to the owner-only file `<data_dir>/.aura-store-key` and
//!   says so at WARN.
//! * **Key file**: when `AURA_STATE_KEY_FILE` is set (the variable
//!   `aura-node` reads), only that file is used, created `0600` on first
//!   start. For servers, CI, tests and portable data directories.
//!
//! Either way the key is 64 hex characters, the format of `aura-node`'s
//! key file, so the two can share one.

use super::LocalStoreError;
use aura_auth::KeychainSecret;
use aura_store_db::seal_key::{create_key_file, decode_key_hex, encode_key_hex};
use aura_store_db::SealCipher;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// Environment variable naming a key file to use instead of the keychain.
pub const ENV_STATE_KEY_FILE: &str = "AURA_STATE_KEY_FILE";

/// The data directory's store id, naming its keychain entry.
const STORE_ID_FILENAME: &str = ".aura-store-id";

/// Fallback key file when the OS keychain is unavailable.
const FALLBACK_KEY_FILENAME: &str = ".aura-store-key";

/// A 256-bit key, zeroized on drop.
pub(crate) type Key = Zeroizing<[u8; 32]>;

/// Where the store's key is read from and created in.
#[derive(Debug)]
pub enum KeySource {
    /// Only this file.
    File(PathBuf),
    /// The OS keychain, with its fallback file.
    Keychain {
        /// The entry.
        secret: KeychainSecret,
        /// The entry's name and fallback file, for messages.
        label: String,
    },
}

impl KeySource {
    /// The source for `data_dir`: [`ENV_STATE_KEY_FILE`] when set, else
    /// the keychain entry of the data directory's store id (created on
    /// first use).
    ///
    /// # Errors
    /// The store id can't be read or created.
    pub fn for_data_dir(data_dir: &Path) -> Result<Self, LocalStoreError> {
        if let Some(path) = std::env::var_os(ENV_STATE_KEY_FILE).filter(|v| !v.is_empty()) {
            return Ok(Self::File(PathBuf::from(path)));
        }
        let store_id = store_id(data_dir)?;
        let user = format!("store-key:{store_id}");
        let fallback = data_dir.join(FALLBACK_KEY_FILENAME);
        let label = format!(
            "the OS keychain (entry aura/{user}, or the file {} without a keychain)",
            fallback.display()
        );
        Ok(Self::Keychain {
            secret: KeychainSecret::new(user, fallback),
            label,
        })
    }

    /// Where the key is, for messages. Never the key itself.
    pub fn describe(&self) -> String {
        match self {
            Self::File(path) => format!("the key file {}", path.display()),
            Self::Keychain { label, .. } => label.clone(),
        }
    }

    /// The stored key; `None` when there is none.
    ///
    /// # Errors
    /// The key can't be read, or what is stored is not a key.
    pub fn load(&self) -> Result<Option<Key>, LocalStoreError> {
        let text = match self {
            Self::File(path) => match std::fs::read_to_string(path) {
                Ok(text) => Some(Zeroizing::new(text)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(self.error(format!("reading: {e}"))),
            },
            Self::Keychain { secret, .. } => secret
                .read()
                .map_err(|e| self.error(e.to_string()))?
                .map(Zeroizing::new),
        };
        text.map(|text| {
            decode_key_hex(&text).map_err(|e| self.error(format!("not a valid key: {e}")))
        })
        .transpose()
    }

    /// Make a new key, store it, and read it back to prove it is kept.
    ///
    /// # Errors
    /// Storing or reading back failed, or read back something else.
    pub fn create(&self) -> Result<Key, LocalStoreError> {
        let key = SealCipher::generate_key();
        let text = encode_key_hex(&key);
        match self {
            Self::File(path) => {
                if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                    std::fs::create_dir_all(dir)
                        .map_err(|e| self.error(format!("creating its folder: {e}")))?;
                }
                create_key_file(path, text.as_bytes())
                    .map_err(|e| self.error(format!("creating: {e}")))?;
            }
            Self::Keychain { secret, .. } => {
                secret.save(&text).map_err(|e| self.error(e.to_string()))?;
            }
        }
        match self.load()? {
            Some(kept) if *kept == *key => Ok(key),
            _ => Err(self.error("the new key did not read back".to_string())),
        }
    }

    fn error(&self, detail: String) -> LocalStoreError {
        LocalStoreError::Key {
            location: self.describe(),
            detail,
        }
    }
}

/// The store id in `data_dir`, created when there is none.
fn store_id(data_dir: &Path) -> Result<String, LocalStoreError> {
    let path = data_dir.join(STORE_ID_FILENAME);
    let failed = |detail: String| LocalStoreError::Key {
        location: path.display().to_string(),
        detail,
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            return uuid::Uuid::parse_str(text.trim())
                .map(|id| id.to_string())
                .map_err(|e| failed(format!("not a store id: {e}")));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(failed(format!("reading: {e}"))),
    }
    let id = uuid::Uuid::new_v4().to_string();
    std::fs::create_dir_all(data_dir).map_err(|e| failed(format!("creating its folder: {e}")))?;
    // Not secret, but written once: `create_new` keeps a racing writer's id.
    match create_key_file(&path, id.as_bytes()) {
        Ok(()) => Ok(id),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => store_id(data_dir),
        Err(e) => Err(failed(format!("creating: {e}"))),
    }
}
