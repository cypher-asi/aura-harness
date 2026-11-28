//! Sealing the vault's fallback files, for machines with no keychain.
//!
//! A token written to its fallback file goes in the local store's sealed
//! envelope (AES-256-GCM, [`SealCipher`], #93) instead of in plain text.
//! The key is 64 hex characters ([`aura_store_db::seal_key`]) in a key
//! file of its own:
//!
//! - `AURA_STATE_KEY_FILE` when set, the same file the local store and
//!   `aura-node` use, so a portable setup keeps one key;
//! - else `<vault root>/auth/vault.key`, created `0600` the first time a
//!   token is sealed.
//!
//! ## Invariants
//!
//! - Nothing is written in plain text: a save seals, and a plain file
//!   left from before is replaced by its sealed copy only after that copy
//!   was read back and opened to the same token ([`FileSeal::migrate`]).
//!   Until then the plain file stays and is still read, so no sign-in is
//!   ever lost; the next read tries again.
//! - A key is created only to seal, never to read: a sealed file whose
//!   key is missing or different is an error that says so, not an empty
//!   slot, and nothing overwrites it.
//!
//! The key file sits on the same disk as the tokens when the keychain is
//! missing, so this keeps tokens out of plain-text files, backups and
//! logs of them rather than out of reach of the user's own account.

use crate::error::AuthError;
use aura_store_db::seal_key::{create_key_file, decode_key_hex, encode_key_hex};
use aura_store_db::SealCipher;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// The variable naming a key file to seal with instead of the vault's own.
pub(crate) const ENV_KEY_FILE: &str = "AURA_STATE_KEY_FILE";

/// How a vault's fallback files are sealed: with the key in `key_file`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileSeal {
    key_file: PathBuf,
}

impl FileSeal {
    /// The seal of the vault at `root`: [`ENV_KEY_FILE`] when `from_env`
    /// and it is set, else `<root>/auth/vault.key`.
    pub(crate) fn for_root(root: &Path, from_env: bool) -> Self {
        let named = from_env
            .then(|| std::env::var_os(ENV_KEY_FILE))
            .flatten()
            .filter(|value| !value.is_empty());
        let key_file = named.map_or_else(|| root.join("auth").join("vault.key"), PathBuf::from);
        Self { key_file }
    }

    /// The seal with the key in `key_file`.
    #[cfg(test)]
    pub(crate) fn with_key_file(key_file: PathBuf) -> Self {
        Self { key_file }
    }

    fn key(&self) -> Result<Option<Zeroizing<[u8; 32]>>, AuthError> {
        let text = match std::fs::read_to_string(&self.key_file) {
            Ok(text) => Zeroizing::new(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(self.io(e)),
        };
        decode_key_hex(&text)
            .map(Some)
            .map_err(|e| AuthError::CredentialSealed {
                path: self.key_file.clone(),
                reason: format!("the key file is not a key: {e}"),
            })
    }

    /// The key, made now when there is none.
    fn key_or_create(&self) -> Result<Zeroizing<[u8; 32]>, AuthError> {
        if let Some(key) = self.key()? {
            return Ok(key);
        }
        if let Some(dir) = self.key_file.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| self.io(e))?;
        }
        let key = SealCipher::generate_key();
        match create_key_file(&self.key_file, encode_key_hex(&key).as_bytes()) {
            Ok(()) => Ok(key),
            // Another process made one first: use that.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => self
                .key()?
                .ok_or_else(|| self.io(std::io::Error::from(std::io::ErrorKind::NotFound))),
            Err(e) => Err(self.io(e)),
        }
    }

    /// `secret` sealed, making the key when there is none.
    pub(crate) fn seal(&self, secret: &str) -> Result<Vec<u8>, AuthError> {
        let key = self.key_or_create()?;
        SealCipher::new(&key)
            .seal(secret.as_bytes())
            .map_err(|e| AuthError::CredentialSealed {
                path: self.key_file.clone(),
                reason: format!("sealing failed: {e}"),
            })
    }

    /// The secret `path` holds as `bytes`: opened when sealed, as it is
    /// when it is a plain file from before.
    pub(crate) fn open(&self, path: &Path, bytes: &[u8]) -> Result<Opened, AuthError> {
        if !SealCipher::is_sealed(bytes) {
            let text = String::from_utf8_lossy(bytes).into_owned();
            return Ok(Opened::Plain(Zeroizing::new(text)));
        }
        let sealed = |reason: String| AuthError::CredentialSealed {
            path: path.to_path_buf(),
            reason,
        };
        let Some(key) = self.key()? else {
            return Err(sealed(format!(
                "its key file {} is missing",
                self.key_file.display()
            )));
        };
        let plain = SealCipher::new(&key).open(bytes).map_err(|_| {
            sealed(format!(
                "it does not open with the key in {}",
                self.key_file.display()
            ))
        })?;
        String::from_utf8(plain)
            .map(Opened::Sealed)
            .map_err(|_| sealed("it holds no text".to_string()))
    }

    /// Replace the plain file `path`, holding `plain`, with its sealed
    /// copy: write the copy next to it, read it back and open it to the
    /// same text, then rename it over the plain file. On any failure the
    /// plain file stays as it was.
    pub(crate) fn migrate(
        &self,
        path: &Path,
        plain: &str,
        write: impl Fn(&Path, &[u8]) -> std::io::Result<()>,
    ) -> Result<(), AuthError> {
        let staged = staged_path(path);
        let io = |e: std::io::Error| AuthError::CredentialIo {
            path: staged.clone(),
            source: e,
        };
        write(&staged, &self.seal(plain)?).map_err(io)?;
        let back = std::fs::read(&staged).map_err(io)?;
        match self.open(&staged, &back)? {
            Opened::Sealed(text) if text == plain => {}
            _ => {
                let _ = std::fs::remove_file(&staged);
                return Err(AuthError::CredentialSealed {
                    path: staged,
                    reason: "its sealed copy did not read back the same".to_string(),
                });
            }
        }
        std::fs::rename(&staged, path).map_err(io)
    }

    fn io(&self, source: std::io::Error) -> AuthError {
        AuthError::CredentialIo {
            path: self.key_file.clone(),
            source,
        }
    }
}

/// What a fallback file held.
pub(crate) enum Opened {
    /// Sealed, now opened
    Sealed(String),
    /// A plain file from before sealing, to migrate
    Plain(Zeroizing<String>),
}

/// Where a migration stages the sealed copy of `path`.
pub(crate) fn staged_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".sealing");
    path.with_file_name(name)
}
