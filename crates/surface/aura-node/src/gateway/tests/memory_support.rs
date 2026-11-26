//! HTTP helpers shared by the memory gateway tests.

use super::support::authed_request;
use axum::body::Body;
use axum::response::Response;
use axum::Router;
use tower::util::ServiceExt;

/// Query string that places a request in the `project-x` / `user-u` partition.
pub(super) const PROJECT_X_QUERY: &str = "project_id=project-x&user_id=user-u";

/// Query string that places a request in the `project-y` / `user-u` partition.
pub(super) const PROJECT_Y_QUERY: &str = "project_id=project-y&user_id=user-u";

/// Fact request body carrying an explicit, active continuity scope.
pub(super) fn scoped_fact_body(key: &str, value: &str, scope: &str) -> serde_json::Value {
    serde_json::json!({
        "key": key,
        "value": value,
        "continuity": {
            "scope": scope,
            "status": "active",
            "sensitivity": "normal",
            "pinned": false,
            "provenance": {}
        }
    })
}

/// Send an authenticated request with a JSON body.
pub(super) async fn send_json(
    app: &Router,
    method: &str,
    uri: &str,
    body: &serde_json::Value,
) -> Response {
    let req = authed_request()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap()
}

/// Send an authenticated `POST` with a JSON body.
pub(super) async fn post_json(app: &Router, uri: &str, body: &serde_json::Value) -> Response {
    send_json(app, "POST", uri, body).await
}

/// Send an authenticated `GET` with an empty body.
pub(super) async fn get(app: &Router, uri: &str) -> Response {
    let req = authed_request().uri(uri).body(Body::empty()).unwrap();
    app.clone().oneshot(req).await.unwrap()
}

/// Read a response body and parse it as JSON.
pub(super) async fn json_body(resp: Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Collect the `key` field of every fact in a memory snapshot packet.
pub(super) fn fact_keys(packet: &serde_json::Value) -> Vec<&str> {
    packet["facts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|fact| fact["key"].as_str())
        .collect()
}
