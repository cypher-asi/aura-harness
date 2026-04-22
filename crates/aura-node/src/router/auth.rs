//! Router authentication helpers.
//!
//! Centralized bearer-token enforcement so that mutating / privileged
//! routes all share the exact same extraction rules and failure shape.
//! Added in Wave 5 (T1.4) to close the "read token, ignore value" pattern
//! that existed on `/stream/automaton/:id`. Phase 4 of the security
//! audit (this file's current state) swapped the presence-only check
//! for a constant-time comparison against a real per-launch secret —
//! matching the `src/api_server.rs` pattern landed in phase 3.

use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::RouterState;

/// Extract a non-empty Bearer token from the `Authorization` header and
/// compare it against `expected` in constant time.
///
/// Returns `Err(StatusCode::UNAUTHORIZED)` when the header is missing,
/// malformed, carries an empty token, or carries a token that does not
/// match `expected`. Also rejects when `expected` itself is empty so a
/// misconfigured deployment (no token loaded) cannot be authenticated
/// with a random value.
///
/// The presented token is returned on success so downstream handlers
/// that want to log a principal (never the raw token) can see it was
/// validated. Because the compare is constant-time, a timing observer
/// cannot distinguish "wrong length" from "wrong bytes at position N".
pub(crate) fn require_bearer(headers: &HeaderMap, expected: &str) -> Result<String, StatusCode> {
    // Refuse to auth when the server has no secret loaded — otherwise
    // any caller who submits `Bearer ""` would compare against `""`
    // and succeed. The separate check also localises the "empty
    // secret" bug report, which is more actionable than a generic
    // 401.
    if expected.is_empty() {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let value = headers
        .get(axum::http::header::AUTHORIZATION)
        .ok_or(StatusCode::UNAUTHORIZED)?
        .to_str()
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    let token = value
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?
        .trim();

    if token.is_empty() {
        return Err(StatusCode::UNAUTHORIZED);
    }

    if !constant_time_eq(token.as_bytes(), expected.as_bytes()) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(token.to_string())
}

/// Strongly-typed Bearer token extracted by the auth middleware.
///
/// Inserted into request extensions by [`require_bearer_mw`] so downstream
/// handlers can read the validated token without having to re-parse the
/// `Authorization` header. Wrapped in a newtype so it can't be confused
/// with any other `String` extension.
#[derive(Debug, Clone)]
#[allow(dead_code)] // read by future handlers that want to surface the
                    // authenticated principal
pub(crate) struct BearerToken(pub String);

/// Axum middleware enforcing that every request presents the expected
/// Bearer token in its `Authorization` header.
///
/// The expected token is pulled from [`RouterState::config`], which was
/// populated by `Node::run` from `AURA_NODE_AUTH_TOKEN`, a persisted
/// `$data_dir/auth_token` file, or a freshly-minted per-launch random
/// value. The comparison is constant-time so a network-adjacent
/// attacker cannot probe the secret byte-by-byte via response timing.
///
/// Returns `401 UNAUTHORIZED` when [`require_bearer`] rejects the
/// request. On success the validated token is cloned into the request
/// extensions as a [`BearerToken`] before the request is forwarded to
/// `next`.
pub(crate) async fn require_bearer_mw(
    State(state): State<RouterState>,
    mut request: Request,
    next: Next,
) -> Response {
    let token = match require_bearer(request.headers(), &state.config.auth_token) {
        Ok(t) => t,
        Err(status) => return status.into_response(),
    };
    request.extensions_mut().insert(BearerToken(token));
    next.run(request).await
}

/// Constant-time byte-slice compare.
///
/// Pads the shorter side by folding its length into the accumulator so
/// timing cannot leak whether lengths matched. An inline implementation
/// is used rather than pulling in the `subtle` crate; the terminal-mode
/// `api_server` landed in phase 3 uses the same shape, so the two
/// servers stay consistent.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    // `len_xor` is non-zero iff the lengths differ. We only care
    // whether the final accumulator is non-zero, so collapsing to a
    // single bit with `(len_xor != 0) as u8` is information-preserving
    // for the equality verdict *and* sidesteps the `usize -> u8`
    // truncation clippy warns about. A timing-observer still learns
    // only "unequal" — not which byte or bit differed.
    let len_xor = a.len() ^ b.len();
    let n = a.len().min(b.len());
    let mut acc: u8 = u8::from(len_xor != 0);
    for i in 0..n {
        acc |= a[i] ^ b[i];
    }
    acc == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    const EXPECTED: &str = "expected-token";

    #[test]
    fn rejects_missing_header() {
        let headers = HeaderMap::new();
        assert_eq!(
            require_bearer(&headers, EXPECTED),
            Err(StatusCode::UNAUTHORIZED)
        );
    }

    #[test]
    fn rejects_non_bearer_scheme() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Basic abc"),
        );
        assert_eq!(
            require_bearer(&headers, EXPECTED),
            Err(StatusCode::UNAUTHORIZED)
        );
    }

    #[test]
    fn rejects_empty_presented_token() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer "),
        );
        assert_eq!(
            require_bearer(&headers, EXPECTED),
            Err(StatusCode::UNAUTHORIZED)
        );
    }

    #[test]
    fn rejects_wrong_token() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer not-the-right-value"),
        );
        assert_eq!(
            require_bearer(&headers, EXPECTED),
            Err(StatusCode::UNAUTHORIZED)
        );
    }

    #[test]
    fn rejects_token_with_different_length() {
        // A shorter token that is a prefix of EXPECTED used to let an
        // attacker probe byte-by-byte via timing; the constant-time
        // path must still reject it with 401 and no length leak.
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer expected"),
        );
        assert_eq!(
            require_bearer(&headers, EXPECTED),
            Err(StatusCode::UNAUTHORIZED)
        );
    }

    #[test]
    fn rejects_when_expected_is_empty() {
        // A misconfigured server that forgot to load a secret must
        // not accept `Bearer <anything>` — otherwise the auth layer
        // would be a no-op.
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer whatever"),
        );
        assert_eq!(require_bearer(&headers, ""), Err(StatusCode::UNAUTHORIZED));
    }

    #[test]
    fn accepts_matching_bearer() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer expected-token"),
        );
        assert_eq!(require_bearer(&headers, EXPECTED), Ok(EXPECTED.to_string()));
    }

    #[test]
    fn constant_time_eq_matches_equal_slices() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"", b"a"));
        assert!(constant_time_eq(b"", b""));
    }
}
