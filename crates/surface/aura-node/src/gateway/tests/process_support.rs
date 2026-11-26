//! Small request helpers shared by the processes, files and secrets tests.

use super::support::authed_request;
use axum::body::{Body, Bytes};
use axum::http::StatusCode;
use axum::Router;
use tower::util::ServiceExt;

/// Send an authed request; a JSON `body` also sets the content type.
/// Returns the status and the raw response body.
pub(super) async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, Bytes) {
    let builder = authed_request().method(method).uri(uri);
    let req = match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, bytes)
}

/// Like [`send`], but parses the response body as JSON.
pub(super) async fn send_json(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let (status, bytes) = send(app, method, uri, body).await;
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// Send an authed request and return only the status.
pub(super) async fn send_status(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> StatusCode {
    send(app, method, uri, body).await.0
}
