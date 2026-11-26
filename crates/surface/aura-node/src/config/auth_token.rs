//! Per-install bearer secret: env override, persisted file, or freshly minted.

use std::path::Path;

/// File name used for persisted per-install auth tokens under `$data_dir`.
const AUTH_TOKEN_FILENAME: &str = "auth_token";

/// Resolve the bearer secret the node will enforce on protected routes.
///
/// Source order, matching the Wave 5 / phase-4 security hardening spec:
///
/// 1. `AURA_NODE_AUTH_TOKEN` environment variable — if present and
///    non-empty, used verbatim. Not persisted and not printed; the
///    operator is assumed to have deliberately set it.
/// 2. `$data_dir/auth_token` file — if present and non-empty, reused
///    so operators don't get a fresh token on every restart. On Unix
///    the file's mode must be `0600`; anything more permissive is
///    treated as tampered and a fresh token is minted over the top.
/// 3. Otherwise a new 32-byte random value is minted (hex-encoded to
///    64 chars), written to `$data_dir/auth_token` with mode `0600`
///    on Unix, and printed **once** to stderr so the launching shell
///    can copy it into client tooling.
///
/// Errors bubble up as `io::Error`; the caller (`Node::run`) decides
/// whether to abort startup or proceed with a best-effort fallback.
pub fn resolve_auth_token(data_dir: &Path) -> std::io::Result<String> {
    if let Ok(env_val) = std::env::var("AURA_NODE_AUTH_TOKEN") {
        let trimmed = env_val.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    let token_path = data_dir.join(AUTH_TOKEN_FILENAME);
    if let Some(existing) = read_existing_auth_token(&token_path) {
        return Ok(existing);
    }

    let token = mint_auth_token();
    std::fs::create_dir_all(data_dir)?;
    write_auth_token(&token_path, &token)?;
    // Log-once to stderr matches the `jupyter notebook` UX: a human
    // can copy the token into curl/browser tooling on first launch.
    // Do NOT promote this to stdout, the tracing logger, or any file
    // other than `$data_dir/auth_token` — the token is only as strong
    // as its handling.
    eprintln!(
        "aura-node auth token: {token} (store in client: export AURA_NODE_AUTH_TOKEN={token})"
    );
    Ok(token)
}

/// Try to reuse an on-disk auth token.
///
/// Returns `None` when the file is missing, empty, unreadable, or —
/// on Unix — has permissions more permissive than `0600`. In each of
/// those cases the caller mints a fresh token instead so an attacker
/// who managed to drop a world-readable file can't pin the secret.
fn read_existing_auth_token(path: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(path).ok()?;
        // Reject any group / world bits — `0o077` tests "other than
        // owner-rw"; anything matching is considered tampered.
        if meta.permissions().mode() & 0o077 != 0 {
            return None;
        }
    }
    Some(trimmed.to_string())
}

/// Mint a 32-byte random hex token (~256 bits of entropy).
///
/// Uses two `uuid::Uuid::new_v4()` values concatenated so we don't have
/// to pull in `rand` just for this — `uuid` is already a workspace
/// dependency and v4 UUIDs are cryptographically random on every
/// supported platform. Mirrors the pattern used by the terminal-mode
/// `api_server` minted in phase 3.
fn mint_auth_token() -> String {
    let a = uuid::Uuid::new_v4().simple().to_string();
    let b = uuid::Uuid::new_v4().simple().to_string();
    format!("{a}{b}")
}

/// Persist `token` at `path` with tight permissions.
///
/// On Unix the file is created with mode `0600` via `OpenOptions::mode`
/// so we never briefly expose a world-readable file. On Windows we fall
/// back to a plain write; NTFS ACLs are inherited from the parent data
/// directory and the token file never contains anything a Windows user
/// wouldn't already have access to (everything runs under their own
/// account).
fn write_auth_token(path: &Path, token: &str) -> std::io::Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(token.as_bytes())?;
        f.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        f.write_all(token.as_bytes())?;
        f.sync_all()?;
    }
    Ok(())
}
