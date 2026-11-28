//! The non-secret `.aura-sealed` marker: "this data dir's store is
//! sealed", plus a key check that tells a wrong key apart before any
//! value is read.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use super::SealMigrationError;
use crate::seal::{SealCipher, SEAL_VERSION};

/// File name of the marker in the data directory.
pub const SEALED_MARKER_FILENAME: &str = ".aura-sealed";

/// What the key check seals. Not secret: it only proves the key opens it.
const KEY_CHECK_PLAINTEXT: &[u8] = b"aura-store-key-check";

/// Largest marker read; a real one is a few hundred bytes.
const MAX_MARKER_BYTES: u64 = 64 * 1024;

/// Contents of the `.aura-sealed` marker file.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct SealedMarker {
    /// Sealed on-disk format version (see [`crate::seal`]).
    pub version: u8,
    /// AEAD algorithm identifier.
    pub cipher: String,
    /// Key id the state is sealed under (non-secret).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    /// Hex of a sealed known value; opening it proves the key is right.
    /// Absent in markers written before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_check: Option<String>,
}

/// `<data_dir>/.aura-sealed`.
#[must_use]
pub fn marker_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SEALED_MARKER_FILENAME)
}

/// Write the marker for a store sealed under `cipher`, atomically
/// (temp file, sync, rename), so a crash leaves the old marker or the
/// new one, never half of one.
///
/// # Errors
/// The key check can't be sealed, or the file can't be written.
pub fn write_marker(
    data_dir: &Path,
    key_id: Option<&str>,
    cipher: &SealCipher,
) -> Result<(), SealMigrationError> {
    let path = marker_path(data_dir);
    let check = cipher
        .seal(KEY_CHECK_PLAINTEXT)
        .map_err(|e| SealMigrationError::Marker {
            path: path.clone(),
            detail: format!("sealing the key check: {e}"),
        })?;
    let marker = SealedMarker {
        version: SEAL_VERSION,
        cipher: "aes-256-gcm".to_string(),
        key_id: key_id.map(str::to_string),
        key_check: Some(hex(&check)),
    };
    let bytes = serde_json::to_vec_pretty(&marker).map_err(|e| SealMigrationError::Marker {
        path: path.clone(),
        detail: format!("encoding: {e}"),
    })?;
    std::fs::create_dir_all(data_dir)
        .map_err(|e| SealMigrationError::io("creating the data directory", data_dir, e))?;
    write_atomically(&path, &bytes)
        .map_err(|e| SealMigrationError::io("writing the sealed marker", &path, e))?;
    super::fsync_dir_best_effort(data_dir);
    Ok(())
}

/// The marker in `data_dir`; `None` when there is none.
///
/// # Errors
/// It exists but can't be read or parsed.
pub fn read_marker(data_dir: &Path) -> Result<Option<SealedMarker>, SealMigrationError> {
    let path = marker_path(data_dir);
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(SealMigrationError::io(
                "reading the sealed marker",
                &path,
                e,
            ))
        }
    };
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(file, MAX_MARKER_BYTES), &mut bytes)
        .map_err(|e| SealMigrationError::io("reading the sealed marker", &path, e))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| SealMigrationError::Marker {
            path,
            detail: format!("not a valid marker: {e}"),
        })
}

/// Check `cipher` against the marker in `data_dir`.
///
/// Passes when the marker's key check opens with `cipher`, and when the
/// marker predates key checks (nothing to compare against).
///
/// # Errors
/// [`SealMigrationError::WrongKey`] when the check does not open; marker
/// read errors.
pub fn check_key(data_dir: &Path, cipher: &SealCipher) -> Result<(), SealMigrationError> {
    let Some(marker) = read_marker(data_dir)? else {
        return Ok(());
    };
    let Some(check) = marker.key_check else {
        return Ok(());
    };
    let wrong_key = || SealMigrationError::WrongKey {
        marker: marker_path(data_dir),
    };
    let sealed = unhex(&check).ok_or_else(wrong_key)?;
    match cipher.open(&sealed) {
        Ok(plain) if plain == KEY_CHECK_PLAINTEXT => Ok(()),
        _ => Err(wrong_key()),
    }
}

/// Remove the marker; absent is fine.
pub(super) fn remove_marker(data_dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(marker_path(data_dir)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    let text = text.as_bytes();
    if text.len() % 2 != 0 {
        return None;
    }
    text.chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(pair, 16).ok()
        })
        .collect()
}
