use super::process_support::*;
use super::support::*;
use crate::gateway::*;
use axum::body::Body;
use axum::http::Request;
use axum::http::StatusCode;
use tower::util::ServiceExt;

// ============================================================================
// Secrets vault (Swarm TEE phase 6)
// ============================================================================

/// Full CRUD roundtrip over the HTTP surface: PUT → GET (reveal) →
/// GET (default, redacted) → DELETE → 404.
#[tokio::test]
async fn test_secrets_crud_roundtrip() {
    let state = test_router_state_with_managers();
    let app = create_router(state);
    let uri = "/secrets/stripe-key";

    // Create.
    let body = serde_json::json!({ "value": "sk-live-12345", "description": "stripe key" });
    let (status, json) = send_json(&app, "PUT", uri, Some(body)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["ok"], true);
    assert_eq!(json["secret"]["name"], "stripe-key");
    assert_eq!(json["secret"]["description"], "stripe key");
    assert!(
        json["secret"].get("value").is_none(),
        "PUT response must not echo the value"
    );

    // GET with ?reveal=true returns the value (the in-VM read path).
    let (status, json) = send_json(&app, "GET", "/secrets/stripe-key?reveal=true", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["secret"]["value"], "sk-live-12345");

    // GET without reveal returns metadata only.
    let (status, bytes) = send(&app, "GET", uri, None).await;
    assert_eq!(status, StatusCode::OK);
    let raw = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        !raw.contains("sk-live-12345"),
        "non-reveal GET must not include the value: {raw}"
    );

    // DELETE, then the secret is gone.
    assert_eq!(send_status(&app, "DELETE", uri, None).await, StatusCode::OK);
    assert_eq!(
        send_status(&app, "GET", uri, None).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send_status(&app, "DELETE", uri, None).await,
        StatusCode::NOT_FOUND
    );
}

/// The list endpoint must never return values, only names + metadata.
#[tokio::test]
async fn test_secrets_list_never_returns_values() {
    let state = test_router_state_with_managers();
    let app = create_router(state);

    for (name, value) in [("alpha", "value-alpha-xyz"), ("beta", "value-beta-xyz")] {
        let body = serde_json::json!({ "value": value });
        let req = authed_request()
            .method("PUT")
            .uri(format!("/secrets/{name}"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    let req = authed_request()
        .uri("/secrets")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let raw = String::from_utf8(bytes.to_vec()).unwrap();
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap();

    let secrets = json["secrets"].as_array().unwrap();
    assert_eq!(secrets.len(), 2);
    assert_eq!(secrets[0]["name"], "alpha");
    assert_eq!(secrets[1]["name"], "beta");
    assert!(
        !raw.contains("value-alpha-xyz") && !raw.contains("value-beta-xyz"),
        "list response must never contain secret values: {raw}"
    );
}

/// Secrets routes sit on the protected sub-router: no bearer → 401.
#[tokio::test]
async fn test_secrets_require_bearer() {
    let state = test_router_state_with_managers();
    let app = create_router(state);

    for (method, uri) in [
        ("GET", "/secrets"),
        ("GET", "/secrets/some-name"),
        ("DELETE", "/secrets/some-name"),
    ] {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {uri} must require a bearer token"
        );
    }
}

/// Invalid names are rejected before touching the store.
#[tokio::test]
async fn test_secrets_invalid_name_rejected() {
    let state = test_router_state_with_managers();
    let app = create_router(state);

    let body = serde_json::json!({ "value": "v" });
    let req = authed_request()
        .method("PUT")
        .uri("/secrets/bad%20name")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
