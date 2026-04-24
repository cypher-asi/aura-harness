//! Credential redaction + auth-URL injection helpers.
//!
//! [`build_auth_url`] embeds a JWT into the user-info component of an
//! `https://` push URL so the credential lives only on the in-memory
//! command line — never in `.git/config`. [`redact_url`] is the
//! complementary scrubber: it replaces any user-info with `***` so the
//! crate's structured tracing never leaks the JWT.

use super::GitToolError;

/// Inject `x-token:<jwt>` into `remote_url`'s user-info component.
///
/// Accepts `https://host[:port]/path` URLs. Any other shape — including
/// ssh://, file://, scp-style `git@host:path` — is rejected with
/// [`GitToolError::InvalidUrl`]. The caller is responsible for stripping
/// existing credentials; we do not attempt to merge them.
pub(super) fn build_auth_url(remote_url: &str, jwt: &str) -> Result<String, GitToolError> {
    let Some((scheme, rest)) = remote_url.split_once("://") else {
        return Err(GitToolError::InvalidUrl(
            "remote URL must contain '://'".into(),
        ));
    };
    if scheme != "https" && scheme != "http" {
        return Err(GitToolError::InvalidUrl(format!(
            "unsupported scheme '{scheme}' (expected https or http)"
        )));
    }
    if rest.is_empty() {
        return Err(GitToolError::InvalidUrl("remote URL is empty".into()));
    }
    // Strip any pre-existing user-info segment so we don't leak
    // credentials or end up with `user:token@newuser:newtoken@host`.
    let without_auth = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
    if without_auth.is_empty() {
        return Err(GitToolError::InvalidUrl("remote URL has no host".into()));
    }
    // Reject obvious control chars in the JWT. The JWT itself is
    // user-provided and ends up on the command line, so make sure
    // no newline / whitespace / shell meta sneaks in — these are not
    // meaningful in a JWT anyway.
    for c in jwt.chars() {
        if c.is_ascii_whitespace() || c == '@' || c == '#' || c.is_control() {
            return Err(GitToolError::InvalidUrl(format!(
                "auth token contains disallowed character: {c:?}"
            )));
        }
    }
    Ok(format!("{scheme}://x-token:{jwt}@{without_auth}"))
}

/// Mask any user-info portion of `url` to `***` so structured tracing
/// never leaks a JWT. Non-URL strings pass through unchanged.
pub(super) fn redact_url(url: &str) -> String {
    if let Some((scheme, rest)) = url.split_once("://") {
        if let Some((_, host)) = rest.rsplit_once('@') {
            return format!("{scheme}://***@{host}");
        }
        return format!("{scheme}://{rest}");
    }
    url.to_string()
}
