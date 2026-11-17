//! Stable installation host id sent as `ext_agent_host_id`.

use aura_core_auth::AuthError;
use std::io::Write;
use std::path::Path;

/// Read the host id at `path`, creating a new `urn:uuid:` id (mode
/// 0600 on Unix) when absent.
///
/// # Errors
///
/// Returns [`AuthError::Storage`] when the file cannot be read or
/// created, or holds something other than a `urn:uuid:` id.
pub fn load_or_create_host_id(path: &Path) -> Result<String, AuthError> {
    if let Ok(value) = std::fs::read_to_string(path) {
        let value = value.trim().to_string();
        if value.starts_with("urn:uuid:") {
            return Ok(value);
        }
        return Err(AuthError::Storage(
            "Invalid ChatGPT installation host id".into(),
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|_| AuthError::Storage("Host id directory unavailable".into()))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let value = format!("urn:uuid:{}", uuid::Uuid::new_v4());
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(value.as_bytes())
                .map_err(|_| AuthError::Storage("Host id could not be saved".into()))?;
            Ok(value)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => load_or_create_host_id(path),
        Err(_) => Err(AuthError::Storage("Host id storage unavailable".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_id_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("chatgpt-host");
        let first = load_or_create_host_id(&path).unwrap();
        assert!(first.starts_with("urn:uuid:"));
        assert_eq!(load_or_create_host_id(&path).unwrap(), first);
    }

    #[test]
    fn rejects_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chatgpt-host");
        std::fs::write(&path, "nope").unwrap();
        assert!(load_or_create_host_id(&path).is_err());
    }
}
