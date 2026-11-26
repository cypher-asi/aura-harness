use super::support::*;
use crate::gateway::*;
use aura_core_types::AgentId;
use axum::body::Body;
use axum::http::StatusCode;
use std::sync::Arc;
use tower::util::ServiceExt;

// ============================================================================
// Phase 9 (security audit): DoS-protection tests — body limits, rate
// limiting, concurrency caps.
// ============================================================================

/// `POST /tx` with a body larger than the 1 MiB global cap must be
/// refused at the layer level before the handler runs. axum surfaces
/// `DefaultBodyLimit` violations as `413 Payload Too Large`.
#[tokio::test]
async fn test_tx_rejects_body_over_one_mib() {
    let store = create_test_store();
    let state = test_router_state(store);
    let app = create_router(state);

    // 1 MiB + 1 byte of arbitrary payload. The body doesn't have to
    // be valid JSON — the body-limit layer trips before serde runs.
    let oversize = vec![b'x'; 1024 * 1024 + 1];
    let req = authed_request()
        .method("POST")
        .uri("/tx")
        .header("content-type", "application/json")
        .body(Body::from(oversize))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "/tx bodies over 1 MiB must be rejected with 413 before reaching the handler"
    );
}

/// Flooding `/tx` from a single synthetic peer must trigger the
/// strict 5/sec burst-10 governor layer, producing at least one
/// `429 Too Many Requests` response within 100 rapid requests.
#[tokio::test]
async fn test_tx_rate_limit_returns_429_under_flood() {
    let store = create_test_store();
    let state = test_router_state(store);
    let app = create_router(state);

    let agent_id = AgentId::generate();
    let payload_b64 =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "rate-limit");
    let body = serde_json::json!({
        "agent_id": agent_id.to_hex(),
        "kind": "user_prompt",
        "payload": payload_b64,
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();

    let mut saw_429 = false;
    for _ in 0..100 {
        // Fixed peer — all 100 requests share one rate-limit bucket.
        let mut req = authed_request()
            .method("POST")
            .uri("/tx")
            .header("content-type", "application/json")
            .body(Body::from(body_bytes.clone()))
            .unwrap();
        inject_fake_peer(&mut req, 42);

        let resp = app.clone().oneshot(req).await.unwrap();
        if resp.status() == StatusCode::TOO_MANY_REQUESTS {
            saw_429 = true;
            break;
        }
    }
    assert!(
        saw_429,
        "100 rapid /tx requests from one peer must trip the 5/sec burst-10 governor"
    );
}

/// Pure-unit test for the WS slot helper: a semaphore of size N lets
/// through N concurrent acquires; the (N+1)th fails. The production
/// handlers short-circuit to `503` on the `None` return.
#[test]
fn test_ws_slot_semaphore_rejects_over_capacity() {
    use crate::gateway::handlers::run_ws::try_acquire_ws_slot;
    use tokio::sync::Semaphore;

    let sem = Arc::new(Semaphore::new(3));

    let p1 = try_acquire_ws_slot(&sem).expect("first acquire should succeed");
    let p2 = try_acquire_ws_slot(&sem).expect("second acquire should succeed");
    let p3 = try_acquire_ws_slot(&sem).expect("third acquire should succeed");

    assert!(
        try_acquire_ws_slot(&sem).is_none(),
        "fourth acquire past capacity must return None (handler returns 503)"
    );

    // Releasing one permit makes the slot available again.
    drop(p1);
    let p4 = try_acquire_ws_slot(&sem).expect("slot must free after drop");

    drop(p2);
    drop(p3);
    drop(p4);
    assert_eq!(sem.available_permits(), 3);
}
