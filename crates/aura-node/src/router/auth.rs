//! Router authentication helpers.
//!
//! Centralized bearer-token enforcement so that mutating / privileged
//! routes all share the exact same extraction rules and failure shape.
//! Added in Wave 5 (T1.4) to close the "read token, ignore value" pattern
//! that existed on `/stream/automaton/:id`.

use axum::http::{HeaderMap, StatusCode};

/// Extract a non-empty Bearer token from the `Authorization` header.
///
/// Returns `Err(StatusCode::UNAUTHORIZED)` when the header is missing,
/// malformed, or carries an empty token after the `Bearer ` prefix.
///
/// This helper deliberately does not validate the token against any
/// secret — it only enforces that *a* caller-identifying token was
/// supplied. The downstream handler is responsible for any stronger
/// authorization (scopes, tenancy, etc.).
pub(crate) fn require_bearer(headers: &HeaderMap) -> Result<String, StatusCode> {
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

    Ok(token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn rejects_missing_header() {
        let headers = HeaderMap::new();
        assert_eq!(require_bearer(&headers), Err(StatusCode::UNAUTHORIZED));
    }

    #[test]
    fn rejects_non_bearer_scheme() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Basic abc"),
        );
        assert_eq!(require_bearer(&headers), Err(StatusCode::UNAUTHORIZED));
    }

    #[test]
    fn rejects_empty_token() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer "),
        );
        assert_eq!(require_bearer(&headers), Err(StatusCode::UNAUTHORIZED));
    }

    #[test]
    fn accepts_valid_bearer() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer secret-token"),
        );
        assert_eq!(require_bearer(&headers), Ok("secret-token".to_string()));
    }
}
