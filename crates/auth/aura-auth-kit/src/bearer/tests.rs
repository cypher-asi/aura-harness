use super::*;
use ::http::HeaderValue;

const EXPECTED: &str = "expected-token";

#[test]
fn rejects_missing_header() {
    let headers = HeaderMap::new();
    assert_eq!(
        check_bearer(&headers, EXPECTED),
        Err(StatusCode::UNAUTHORIZED)
    );
}

#[test]
fn rejects_non_bearer_scheme() {
    let mut headers = HeaderMap::new();
    headers.insert(
        ::http::header::AUTHORIZATION,
        HeaderValue::from_static("Basic abc"),
    );
    assert_eq!(
        check_bearer(&headers, EXPECTED),
        Err(StatusCode::UNAUTHORIZED)
    );
}

#[test]
fn rejects_empty_presented_token() {
    let mut headers = HeaderMap::new();
    headers.insert(
        ::http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer "),
    );
    assert_eq!(
        check_bearer(&headers, EXPECTED),
        Err(StatusCode::UNAUTHORIZED)
    );
}

#[test]
fn rejects_wrong_token() {
    let mut headers = HeaderMap::new();
    headers.insert(
        ::http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer not-the-right-value"),
    );
    assert_eq!(
        check_bearer(&headers, EXPECTED),
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
        ::http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer expected"),
    );
    assert_eq!(
        check_bearer(&headers, EXPECTED),
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
        ::http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer whatever"),
    );
    assert_eq!(check_bearer(&headers, ""), Err(StatusCode::UNAUTHORIZED));
}

#[test]
fn accepts_matching_bearer() {
    let mut headers = HeaderMap::new();
    headers.insert(
        ::http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer expected-token"),
    );
    assert_eq!(check_bearer(&headers, EXPECTED), Ok(EXPECTED.to_string()));
}

#[test]
fn bearer_any_accepts_primary_and_fallback() {
    let mut headers = HeaderMap::new();
    headers.insert(
        ::http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer swarm-internal-token"),
    );

    // Fallback (the swarm internal token) authenticates...
    assert_eq!(
        check_bearer_any(&headers, EXPECTED, Some("swarm-internal-token")),
        Ok("swarm-internal-token".to_string())
    );
    // ...and the primary node token still works with a fallback set.
    let mut node_headers = HeaderMap::new();
    node_headers.insert(
        ::http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer expected-token"),
    );
    assert_eq!(
        check_bearer_any(&node_headers, EXPECTED, Some("swarm-internal-token")),
        Ok(EXPECTED.to_string())
    );
}

#[test]
fn bearer_any_rejects_wrong_token_and_empty_fallback() {
    let mut headers = HeaderMap::new();
    headers.insert(
        ::http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer not-a-valid-secret"),
    );
    assert_eq!(
        check_bearer_any(&headers, EXPECTED, Some("swarm-internal-token")),
        Err(StatusCode::UNAUTHORIZED)
    );
    assert_eq!(
        check_bearer_any(&headers, EXPECTED, None),
        Err(StatusCode::UNAUTHORIZED)
    );

    // An empty fallback secret must never authenticate anything
    // (mirrors the empty-expected rule of `check_bearer`).
    let mut empty_headers = HeaderMap::new();
    empty_headers.insert(
        ::http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer "),
    );
    assert_eq!(
        check_bearer_any(&empty_headers, EXPECTED, Some("")),
        Err(StatusCode::UNAUTHORIZED)
    );
}

#[test]
fn constant_time_eq_matches_equal_slices() {
    assert!(constant_time_eq(b"abc", b"abc"));
    assert!(!constant_time_eq(b"abc", b"abd"));
    assert!(!constant_time_eq(b"abc", b"ab"));
    assert!(!constant_time_eq(b"", b"a"));
    assert!(constant_time_eq(b"", b""));
}
