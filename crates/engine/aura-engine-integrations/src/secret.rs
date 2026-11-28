//! Credentials: the redacted [`Secret`], and where they are kept
//! ([`SecretStore`]): sealed on disk ([`SealedSecrets`]) or in memory
//! ([`MemorySecrets`], for tests).
//!
//! ## Invariants
//!
//! - [`SealedSecrets`] writes nothing in plain text: the file is the
//!   local store's AES-256-GCM envelope ([`SealCipher`]) around a JSON
//!   map of integration id to secret. It is replaced atomically
//!   (temporary file, then rename) and is `0600` on Unix.
//! - A file that isn't sealed, or doesn't open with the key, is an
//!   error, never read as empty: overwriting it would lose every stored
//!   credential.

use crate::error::IntegrationError;
use crate::id::IntegrationId;
use aura_store_db::SealCipher;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use zeroize::Zeroizing;

/// The sealed credentials' file name, in the integrations folder.
pub const SECRETS_FILE: &str = "secrets.sealed";

/// A credential. Its `Debug` and `Display` never show it; read it with
/// [`Self::expose`] only to send it to its service.
#[derive(Clone)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    /// `value`, trimmed.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        Self(Zeroizing::new(value.trim().to_string()))
    }

    /// The credential itself.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether it is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(…)")
    }
}

impl std::fmt::Display for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("…")
    }
}

/// Where integrations' credentials are kept. Calls are blocking file
/// IO at most; the registry runs them off the async threads.
pub trait SecretStore: Send + Sync {
    /// The credential stored for `id`.
    ///
    /// # Errors
    /// The store can't be read.
    fn get(&self, id: &IntegrationId) -> Result<Option<Secret>, IntegrationError>;

    /// Store `secret` for `id`, replacing any.
    ///
    /// # Errors
    /// The store can't be written.
    fn put(&self, id: &IntegrationId, secret: &Secret) -> Result<(), IntegrationError>;

    /// Forget `id`'s credential; whether there was one.
    ///
    /// # Errors
    /// The store can't be written.
    fn remove(&self, id: &IntegrationId) -> Result<bool, IntegrationError>;
}

type Map = BTreeMap<String, Zeroizing<String>>;

/// Credentials in memory only, for tests and for a session without a
/// data directory.
#[derive(Default)]
pub struct MemorySecrets {
    map: Mutex<Map>,
}

impl SecretStore for MemorySecrets {
    fn get(&self, id: &IntegrationId) -> Result<Option<Secret>, IntegrationError> {
        Ok(lock(&self.map)
            .get(id.as_str())
            .map(|value| Secret(value.clone())))
    }

    fn put(&self, id: &IntegrationId, secret: &Secret) -> Result<(), IntegrationError> {
        lock(&self.map).insert(id.to_string(), secret.0.clone());
        Ok(())
    }

    fn remove(&self, id: &IntegrationId) -> Result<bool, IntegrationError> {
        Ok(lock(&self.map).remove(id.as_str()).is_some())
    }
}

/// Credentials in one sealed file.
pub struct SealedSecrets {
    path: PathBuf,
    cipher: SealCipher,
    /// Read-modify-write of the file, one at a time in this process
    lock: Mutex<()>,
}

impl SealedSecrets {
    /// The sealed store in `path`, opened with `key`: the local store's
    /// key. Nothing is read until it is used.
    #[must_use]
    pub fn new(path: PathBuf, key: &[u8; 32]) -> Self {
        Self {
            path,
            cipher: SealCipher::new(key),
            lock: Mutex::new(()),
        }
    }

    /// The file it is kept in.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn load(&self) -> Result<Map, IntegrationError> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
            Err(e) => return Err(store_error(&self.path, &e)),
        };
        if !SealCipher::is_sealed(&bytes) {
            return Err(IntegrationError::Store(format!(
                "{} is not sealed; it was left as it is",
                self.path.display()
            )));
        }
        let plain = Zeroizing::new(self.cipher.open(&bytes).map_err(|_| {
            IntegrationError::Store(format!(
                "{} does not open with this data directory's key",
                self.path.display()
            ))
        })?);
        let map: BTreeMap<String, String> = serde_json::from_slice(&plain)
            .map_err(|e| IntegrationError::Store(format!("{}: {e}", self.path.display())))?;
        Ok(map
            .into_iter()
            .map(|(k, v)| (k, Zeroizing::new(v)))
            .collect())
    }

    fn save(&self, map: &Map) -> Result<(), IntegrationError> {
        let plain: BTreeMap<&str, &str> =
            map.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let json = Zeroizing::new(
            serde_json::to_vec(&plain).map_err(|e| IntegrationError::Store(e.to_string()))?,
        );
        let sealed = self
            .cipher
            .seal(&json)
            .map_err(|e| IntegrationError::Store(format!("sealing: {e}")))?;
        write_private(&self.path, &sealed).map_err(|e| store_error(&self.path, &e))
    }

    fn edit<T>(&self, change: impl FnOnce(&mut Map) -> T) -> Result<T, IntegrationError> {
        let _one = lock(&self.lock);
        let mut map = self.load()?;
        let result = change(&mut map);
        self.save(&map)?;
        Ok(result)
    }
}

impl SecretStore for SealedSecrets {
    fn get(&self, id: &IntegrationId) -> Result<Option<Secret>, IntegrationError> {
        let _one = lock(&self.lock);
        Ok(self
            .load()?
            .get(id.as_str())
            .map(|value| Secret(value.clone())))
    }

    fn put(&self, id: &IntegrationId, secret: &Secret) -> Result<(), IntegrationError> {
        self.edit(|map| {
            map.insert(id.to_string(), secret.0.clone());
        })
    }

    fn remove(&self, id: &IntegrationId) -> Result<bool, IntegrationError> {
        self.edit(|map| map.remove(id.as_str()).is_some())
    }
}

/// Write `bytes` to `path` atomically, owner-only on Unix.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}

fn store_error(path: &Path, error: &std::io::Error) -> IntegrationError {
    IntegrationError::Store(format!("{}: {error}", path.display()))
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
