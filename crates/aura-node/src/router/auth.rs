//! Router authentication helpers.
//!
//! Centralized bearer-token enforcement so that mutating / privileged
//! routes all share the exact same extraction rules and failure shape.
//! Added in Wave 5 (T1.4) to close the "read token, ignore value" pattern
//! that existed on `/stream/automaton/:id`.

use axum::{
    extract::Request,
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

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

/// Strongly-typed Bearer token extracted by the auth middleware.
///
/// Inserted into request extensions by [`require_bearer_mw`] so downstream
/// handlers can read the validated token without having to re-parse the
/// `Authorization` header. Wrapped in a newtype so it can't be confused
/// with any other `String` extension.
#[derive(Debug, Clone)]
#[allow(dead_code)] // read by future handlers once phase 4 wires up
// the shared-secret check
pub(crate) struct BearerToken(pub String);

/// Axum middleware enforcing that every request carries a non-empty
/// Bearer token in the `Authorization` header.
///
/// Returns `401 UNAUTHORIZED` when [`require_bearer`] rejects the headers.
/// On success, the validated token is cloned into the request extensions
/// as a [`BearerToken`] before the request is forwarded to `next`.
///
/// This deliberately does not validate the token against a configured
/// secret — that is scheduled for a later phase. For now the middleware
/// matches the existing "presence only" semantics of [`require_bearer`].
pub(crate) async fn require_bearer_mw(mut request: Request, next: Next) -> Response {
    let token = match require_bearer(request.headers()) {
        Ok(t) => t,
        Err(status) => return status.into_response(),
    };
    request.extensions_mut().insert(BearerToken(token));
    next.run(request).await
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
