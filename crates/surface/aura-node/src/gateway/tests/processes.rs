use super::process_support::*;
use super::support::*;
use crate::gateway::*;
use axum::body::Body;
use axum::http::Request;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Router,
};
use std::sync::Arc;
use std::time::Duration;
use tower::util::ServiceExt;

// ============================================================================
// Processes / automations (Swarm TEE phase 7)
// ============================================================================

/// Full CRUD pass over /v1/processes, including enable/disable via PUT.
#[tokio::test]
async fn test_processes_crud_roundtrip() {
    let state = test_router_state_with_managers();
    let app = create_router(state);

    // Create.
    let body = serde_json::json!({
        "name": "nightly-report",
        "description": "summarize the day",
        "cron": "0 3 * * *",
        "prompt": "Summarize yesterday's commits.",
        "config": { "channel": "ops" }
    });
    let (status, json) = send_json(&app, "POST", "/v1/processes", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED);
    let id = json["process"]["id"].as_str().unwrap().to_string();
    assert_eq!(json["process"]["enabled"], true, "enabled defaults to true");
    assert!(
        json["process"]["next_run_at"].is_string(),
        "next_run_at computed on create"
    );

    // List includes the full definition (in-VM API).
    let (status, json) = send_json(&app, "GET", "/v1/processes", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["processes"].as_array().unwrap().len(), 1);
    assert_eq!(
        json["processes"][0]["prompt"],
        "Summarize yesterday's commits."
    );

    // Update: disable + new cron.
    let uri = format!("/v1/processes/{id}");
    let body = serde_json::json!({ "enabled": false, "cron": "*/5 * * * *" });
    let (status, json) = send_json(&app, "PUT", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["process"]["enabled"], false);
    assert_eq!(json["process"]["cron"], "*/5 * * * *");
    // Prompt untouched by a partial update.
    assert_eq!(json["process"]["prompt"], "Summarize yesterday's commits.");

    // Get reflects the update.
    assert_eq!(send_status(&app, "GET", &uri, None).await, StatusCode::OK);

    // Delete, then 404.
    assert_eq!(
        send_status(&app, "DELETE", &uri, None).await,
        StatusCode::OK
    );
    assert_eq!(
        send_status(&app, "GET", &uri, None).await,
        StatusCode::NOT_FOUND
    );
}

type Captured = (String, String, String);

/// Start a stub swarm gateway capturing (agent_id, authorization, body)
/// of every trigger-registration push; returns its address.
async fn spawn_trigger_stub(tx: tokio::sync::mpsc::Sender<Captured>) -> std::net::SocketAddr {
    let stub = Router::new()
        .route(
            "/internal/agents/{agent_id}/process-triggers",
            axum::routing::put(
                |State(tx): State<tokio::sync::mpsc::Sender<Captured>>,
                 Path(agent_id): Path<String>,
                 headers: HeaderMap,
                 body: String| async move {
                    let auth = headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    let _ = tx.send((agent_id, auth, body)).await;
                    StatusCode::OK
                },
            ),
        )
        .with_state(tx);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let stub_addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, stub).await.ok();
    });
    stub_addr
}

/// Router whose trigger registrar pushes to the stub at `stub_addr`.
fn router_with_registrar(stub_addr: std::net::SocketAddr) -> Router {
    use crate::trigger_registrar::{RegistrarTarget, TriggerRegistrar};

    let mut state = test_router_state_with_managers();
    let process_store = state.process_store.clone().unwrap();
    state.trigger_registrar = Some(Arc::new(TriggerRegistrar::new(
        Some(RegistrarTarget {
            base_url: format!("http://{stub_addr}"),
            token: "internal-secret".into(),
            agent_id: "feedc0de".into(),
        }),
        process_store,
    )));
    create_router(state)
}

/// Phase 8: a successful process mutation fires a best-effort push of
/// the trigger-metadata set to the swarm gateway's internal
/// replace-sync endpoint — with the internal bearer token, the
/// configured agent id, and a payload of metadata only (no prompt).
#[tokio::test]
async fn test_process_mutation_pushes_trigger_registration() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Captured>(8);
    let app = router_with_registrar(spawn_trigger_stub(tx).await);

    // Mutate: create a process with an in-TEE prompt + config.
    let body = serde_json::json!({
        "name": "nightly-report",
        "cron": "0 3 * * *",
        "prompt": "IN-TEE-PROMPT: summarize my sealed notes",
        "config": { "channel": "ops" }
    });
    let status = send_status(&app, "POST", "/v1/processes", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED);

    // The push happens on a background task — await its arrival.
    let (agent_id, auth, pushed) = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("registration push must arrive after a mutation")
        .expect("stub channel closed");

    assert_eq!(agent_id, "feedc0de");
    assert_eq!(auth, "Bearer internal-secret");

    // The pushed body is the full desired set: metadata only.
    let payload: serde_json::Value = serde_json::from_str(&pushed).unwrap();
    let entries = payload.as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["cron"], "0 3 * * *");
    assert_eq!(entries[0]["enabled"], true);
    assert!(entries[0]["process_id"].is_string());
    assert!(
        !pushed.contains("IN-TEE-PROMPT") && !pushed.contains("prompt"),
        "prompt must never cross the trust boundary; pushed: {pushed}"
    );
    assert!(!pushed.contains("ops") && !pushed.contains("config"));

    // Delete the process: the replace-sync now pushes an empty set.
    let (_, json) = send_json(&app, "GET", "/v1/processes", None).await;
    let id = json["processes"][0]["id"].as_str().unwrap().to_string();
    let uri = format!("/v1/processes/{id}");
    assert_eq!(
        send_status(&app, "DELETE", &uri, None).await,
        StatusCode::OK
    );

    let (_, _, pushed) = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("delete must push the (now empty) desired set")
        .expect("stub channel closed");
    assert_eq!(pushed.trim(), "[]");
}

/// Invalid cron expressions are rejected with 400 on create and update.
#[tokio::test]
async fn test_processes_invalid_cron_rejected() {
    let state = test_router_state_with_managers();
    let app = create_router(state);

    let body = serde_json::json!({
        "name": "broken",
        "cron": "not a cron",
        "prompt": "do things"
    });
    let req = authed_request()
        .method("POST")
        .uri("/v1/processes")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Create a valid one, then try to update it with a bad cron.
    let body = serde_json::json!({
        "name": "ok", "cron": "0 3 * * *", "prompt": "do things"
    });
    let req = authed_request()
        .method("POST")
        .uri("/v1/processes")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let id = json["process"]["id"].as_str().unwrap();

    let body = serde_json::json!({ "cron": "99 99 99 99 99" });
    let req = authed_request()
        .method("PUT")
        .uri(format!("/v1/processes/{id}"))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// Process routes sit on the protected sub-router: no bearer → 401.
#[tokio::test]
async fn test_processes_require_bearer() {
    let state = test_router_state_with_managers();
    let app = create_router(state);

    for (method, uri) in [
        ("GET", "/v1/processes"),
        ("GET", "/v1/processes/some-id"),
        ("DELETE", "/v1/processes/some-id"),
        ("POST", "/v1/processes/some-id/trigger"),
        ("GET", "/v1/processes/some-id/runs"),
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

/// Poll a process's run history until its single run `run_id` leaves
/// `running`; returns the last observed status.
async fn wait_for_run_completion(app: &Router, id: &str, run_id: &str) -> String {
    let mut last_status = String::new();
    for _ in 0..600 {
        let uri = format!("/v1/processes/{id}/runs");
        let (status, json) = send_json(app, "GET", &uri, None).await;
        assert_eq!(status, StatusCode::OK);
        let runs = json["runs"].as_array().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0]["run_id"], run_id);
        last_status = runs[0]["status"].as_str().unwrap_or_default().to_string();
        if last_status != "running" {
            assert_eq!(
                last_status, "success",
                "run must finish successfully (error: {:?})",
                runs[0]["error"]
            );
            assert!(runs[0]["finished_at"].is_string());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    last_status
}

/// `POST /v1/processes/:id/trigger` executes the process prompt
/// through the real chat-run path (MockProvider model, real driver +
/// kernel) and records a run that transitions Running → Success, with
/// `last_run_at` / `next_run_at` bookkeeping on the definition.
#[tokio::test]
async fn test_process_trigger_executes_run_and_records_history() {
    let (state, _tmp) = test_router_state_with_workspace();
    let app = create_router(state);

    // Create the process.
    let body = serde_json::json!({
        "name": "triggered",
        "cron": "0 4 * * *",
        "prompt": "Run the nightly checks."
    });
    let (status, json) = send_json(&app, "POST", "/v1/processes", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED);
    let id = json["process"]["id"].as_str().unwrap().to_string();

    // Trigger: runs are async → 202 with the run record.
    let uri = format!("/v1/processes/{id}/trigger");
    let (status, json) = send_json(&app, "POST", &uri, None).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let run_id = json["run"]["run_id"].as_str().unwrap().to_string();
    assert_eq!(json["run"]["status"], "running");
    assert_eq!(
        json["event_stream_url"],
        format!("/stream/{run_id}"),
        "triggered runs are attachable through the standard stream route"
    );

    // Poll run history until the MockProvider turn completes.
    let last_status = wait_for_run_completion(&app, &id, &run_id).await;
    assert_eq!(
        last_status, "success",
        "timed out waiting for run completion"
    );

    // Definition bookkeeping: last_run_at stamped, next_run_at fresh.
    let uri = format!("/v1/processes/{id}");
    let (_, json) = send_json(&app, "GET", &uri, None).await;
    assert!(json["process"]["last_run_at"].is_string());
    assert!(json["process"]["next_run_at"].is_string());
}

/// Triggering an unknown process must 404 without spawning anything.
#[tokio::test]
async fn test_process_trigger_unknown_id_404() {
    let (state, _tmp) = test_router_state_with_workspace();
    let app = create_router(state);

    let req = authed_request()
        .method("POST")
        .uri("/v1/processes/00000000-0000-0000-0000-000000000000/trigger")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
