use super::resolve_run_auth_token;
use axum::http::{header, HeaderMap, HeaderValue};

fn headers_with_bearer(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    headers
}

#[test]
fn run_auth_prefers_body_jwt() {
    let headers = headers_with_bearer("transport-or-legacy-jwt");
    assert_eq!(
        resolve_run_auth_token(Some("user-jwt".to_string()), &headers, true).as_deref(),
        Some("user-jwt")
    );
}

#[test]
fn run_auth_ignores_transport_bearer_when_router_auth_is_required() {
    let headers = headers_with_bearer("node-transport-secret");
    assert_eq!(resolve_run_auth_token(None, &headers, true), None);
}

#[test]
fn run_auth_accepts_bearer_fallback_when_router_auth_is_disabled() {
    let headers = headers_with_bearer("legacy-user-jwt");
    assert_eq!(
        resolve_run_auth_token(None, &headers, false).as_deref(),
        Some("legacy-user-jwt")
    );
}
