//! Credential redaction for remote URLs.
//!
//! The git tools never put credentials on a command line: `git push`
//! authenticates through git's own credential configuration. A remote
//! URL can still carry user-info (`https://user:token@host/...`) when a
//! user configured it that way, so [`redact_url`] replaces any
//! user-info with `***` before a URL reaches tracing or a tool result.

/// Mask any user-info portion of `url` to `***`. Non-URL strings pass
/// through unchanged.
pub(super) fn redact_url(url: &str) -> String {
    if let Some((scheme, rest)) = url.split_once("://") {
        if let Some((_, host)) = rest.rsplit_once('@') {
            return format!("{scheme}://***@{host}");
        }
        return format!("{scheme}://{rest}");
    }
    url.to_string()
}
