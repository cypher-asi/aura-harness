//! Text encoding and on-disk storage of a 256-bit [`SealCipher`] key.
//!
//! Shared by `aura-node`'s dev-mode key file and the local app's key
//! sources. The encoding is 64 lowercase hex characters; decoding also
//! accepts uppercase and surrounding whitespace, so hand-written keys
//! work.
//!
//! [`SealCipher`]: crate::SealCipher

use std::io::Write as _;
use std::path::Path;
use zeroize::Zeroizing;

/// Length of a hex-encoded key.
pub const KEY_HEX_LEN: usize = 64;

/// A key could not be decoded from text.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum KeyDecodeError {
    /// Not 64 characters after trimming.
    #[error("key must be {KEY_HEX_LEN} hex characters, found {0}")]
    Length(usize),
    /// A character outside `0-9a-fA-F`.
    #[error("key contains a character that is not a hex digit")]
    NotHex,
}

/// Encode `key` as 64 lowercase hex characters, zeroized on drop.
#[must_use]
pub fn encode_key_hex(key: &[u8; 32]) -> Zeroizing<String> {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = Zeroizing::new(String::with_capacity(KEY_HEX_LEN));
    for byte in key {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

/// Decode a key written by [`encode_key_hex`].
///
/// # Errors
/// [`KeyDecodeError`] when `text` is not exactly 64 hex digits once
/// trimmed.
pub fn decode_key_hex(text: &str) -> Result<Zeroizing<[u8; 32]>, KeyDecodeError> {
    let text = text.trim().as_bytes();
    if text.len() != KEY_HEX_LEN {
        return Err(KeyDecodeError::Length(text.len()));
    }
    let mut out = Zeroizing::new([0u8; 32]);
    for (slot, pair) in out.iter_mut().zip(text.chunks_exact(2)) {
        let (Some(high), Some(low)) = (nibble(pair[0]), nibble(pair[1])) else {
            return Err(KeyDecodeError::NotHex);
        };
        *slot = (high << 4) | low;
    }
    Ok(out)
}

fn nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Create the key file at `path` holding `contents`, readable by its
/// owner only (`0600` on Unix; on Windows it inherits the parent
/// directory's ACL). Fails if the file already exists, so a key is
/// never overwritten.
///
/// # Errors
/// The file exists, or can't be created, written or synced.
pub fn create_key_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        let key = [0xa5u8; 32];
        let text = encode_key_hex(&key);
        assert_eq!(text.len(), KEY_HEX_LEN);
        assert!(text.starts_with("a5a5"));
        assert_eq!(*decode_key_hex(&text).unwrap(), key);
        assert_eq!(
            *decode_key_hex(&format!(" {}\n", text.to_uppercase())).unwrap(),
            key
        );
    }

    #[test]
    fn bad_hex_is_rejected() {
        assert_eq!(
            decode_key_hex("abc").map(|_| ()),
            Err(KeyDecodeError::Length(3))
        );
        assert_eq!(
            decode_key_hex(&"zz".repeat(32)).map(|_| ()),
            Err(KeyDecodeError::NotHex)
        );
    }

    #[test]
    fn key_file_is_private_and_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key");
        create_key_file(&path, b"first").unwrap();
        assert!(create_key_file(&path, b"second").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
