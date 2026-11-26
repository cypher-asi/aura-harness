//! Suites 2 and 4: POST /v1/run validation and WebSocket protocol edge cases.

use crate::support::*;

// ============================================================================
// Suite 2: POST /v1/run validation (Phase A wire shape)
//
// These tests pin the HTTP-side validation that used to live on the
// `InboundMessage::SessionInit` first-frame path before Phase A. The
// chat-session bootstrap helpers in `crates/surface/aura-node/src/session`
// reject bad workspaces / project paths up-front and surface them as
// `ChatRequestError { code: "invalid_workspace", ... }`, which the
// `POST /v1/run` handler maps to HTTP 400.
// ============================================================================

#[tokio::test]
async fn run_validation_workspace_path_traversal_rejected() {
    let server = TestServer::start().await;
    let payload = json!({
        "type": { "kind": "chat", "params": { "conversation_messages": [] } },
        "agent_identity": {},
        "model": {},
        "workspace": { "workspace": "../../../etc" },
        "agent_permissions": common::default_agent_permissions_payload(),
        "agent_capabilities": {},
        "user_id": "e2e-user",
    });

    let resp = post_runtime_request(&server, &payload).await;
    assert_eq!(resp.status(), 400, "traversal workspace should 400");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "invalid_workspace");
    assert!(
        body["error"].as_str().unwrap_or("").contains("..")
            || body["error"].as_str().unwrap_or("").contains("under"),
        "expected '..' or 'under' in error, got: {body}"
    );
}

#[tokio::test]
async fn run_validation_workspace_outside_base_rejected() {
    let server = TestServer::start().await;
    let outside = if cfg!(windows) {
        "C:\\Windows\\Temp\\evil"
    } else {
        "/tmp/evil"
    };
    let payload = json!({
        "type": { "kind": "chat", "params": { "conversation_messages": [] } },
        "agent_identity": {},
        "model": {},
        "workspace": { "workspace": outside },
        "agent_permissions": common::default_agent_permissions_payload(),
        "agent_capabilities": {},
        "user_id": "e2e-user",
    });

    let resp = post_runtime_request(&server, &payload).await;
    assert_eq!(resp.status(), 400, "outside-base workspace should 400");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "invalid_workspace");
}

#[tokio::test]
async fn run_validation_project_path_relative_rejected() {
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("sec-relpath");
    std::fs::create_dir_all(&ws_path).unwrap();

    let payload = json!({
        "type": { "kind": "chat", "params": { "conversation_messages": [] } },
        "agent_identity": {},
        "model": {},
        "workspace": {
            "workspace": ws_path.to_string_lossy(),
            "project_path": "relative/path",
        },
        "agent_permissions": common::default_agent_permissions_payload(),
        "agent_capabilities": {},
        "user_id": "e2e-user",
    });

    let resp = post_runtime_request(&server, &payload).await;
    assert_eq!(resp.status(), 400, "relative project_path should 400");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "invalid_workspace");
    assert!(
        body["error"].as_str().unwrap_or("").contains("absolute"),
        "expected 'absolute' in error, got: {body}"
    );
}

#[tokio::test]
async fn run_validation_project_path_with_dotdot_rejected() {
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("sec-dotdot");
    std::fs::create_dir_all(&ws_path).unwrap();

    let evil = if cfg!(windows) {
        "C:\\foo\\..\\bar"
    } else {
        "/foo/../bar"
    };
    let payload = json!({
        "type": { "kind": "chat", "params": { "conversation_messages": [] } },
        "agent_identity": {},
        "model": {},
        "workspace": {
            "workspace": ws_path.to_string_lossy(),
            "project_path": evil,
        },
        "agent_permissions": common::default_agent_permissions_payload(),
        "agent_capabilities": {},
        "user_id": "e2e-user",
    });

    let resp = post_runtime_request(&server, &payload).await;
    assert_eq!(resp.status(), 400, "..-containing project_path should 400");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "invalid_workspace");
    assert!(
        body["error"].as_str().unwrap_or("").contains(".."),
        "expected '..' in error, got: {body}"
    );
}

// ============================================================================
// WebSocket parse-error coverage (post-Phase-A)
//
// The pre-Phase-A WS handler rejected bad first frames before any
// session existed. After Phase A the WS handler only ever sees an
// already-attached session, so we open a valid chat run first and
// then probe the parse-error paths.
// ============================================================================

#[tokio::test]
async fn ws_proto_unknown_message_type_rejected() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("proto-unknown");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = open_chat_run(&server, &chat_request_payload(&ws_path, None)).await;
    ws.send_raw(r#"{"type": "unknown_type", "data": 42}"#).await;

    let msg = ws.recv_json().await.expect("expected error");
    assert_eq!(msg["type"], "error");
    assert_eq!(msg["code"], "parse_error");
}

#[tokio::test]
async fn ws_proto_empty_json_rejected() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("proto-empty-json");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = open_chat_run(&server, &chat_request_payload(&ws_path, None)).await;
    ws.send_raw("{}").await;

    let msg = ws.recv_json().await.expect("expected error");
    assert_eq!(msg["type"], "error");
    assert_eq!(msg["code"], "parse_error");
}

#[tokio::test]
async fn ws_proto_invalid_json_rejected() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("proto-invalid-json");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = open_chat_run(&server, &chat_request_payload(&ws_path, None)).await;
    ws.send_raw("this is not json at all!!!").await;

    let msg = ws.recv_json().await.expect("expected error");
    assert_eq!(msg["type"], "error");
    assert_eq!(msg["code"], "parse_error");
}

// ============================================================================
// Suite 4: WebSocket Protocol Edge Cases
// ============================================================================

#[tokio::test]
async fn ws_proto_cancel_no_active_turn() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("proto-cancel-idle");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = open_chat_run(&server, &chat_request_payload(&ws_path, None)).await;
    // Cancel with no turn in progress -- should be silently ignored
    ws.send_cancel().await;

    // Verify the session is still alive by sending another message
    ws.send_user_message("hello").await;

    // We should get either a turn response or at least no crash
    let msg = ws.recv_json_timeout(Duration::from_secs(10)).await;
    assert!(
        msg.is_some(),
        "session should remain alive after idle cancel"
    );
}

#[tokio::test]
async fn ws_proto_approval_response_no_crash() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("proto-approval");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = open_chat_run(&server, &chat_request_payload(&ws_path, None)).await;

    ws.send_json(&json!({
        "type": "approval_response",
        "tool_use_id": "fake-tool-id",
        "approved": true
    }))
    .await;

    // Should not crash; verify session is alive
    ws.send_user_message("hello").await;
    let msg = ws.recv_json_timeout(Duration::from_secs(10)).await;
    assert!(msg.is_some(), "session should survive approval_response");
}

#[tokio::test]
async fn ws_proto_multiple_concurrent_sessions() {
    let server = start_mock_server().await;

    let mut session_ids = Vec::new();
    for i in 0..3 {
        let ws_path = server.workspaces_path().join(format!("concurrent-{i}"));
        std::fs::create_dir_all(&ws_path).unwrap();

        let payload = chat_request_payload(&ws_path, None);
        let resp = post_runtime_request(&server, &payload).await;
        let body: Value = resp.json().await.unwrap();
        let run_id = body["run_id"].as_str().unwrap().to_string();
        let mut ws = WsClient::connect(&server.stream_url(&run_id)).await;
        let ready = ws.expect_session_ready().await;
        let sid = ready["session_id"].as_str().unwrap().to_string();
        session_ids.push(sid);
    }

    // All session IDs should be unique
    let mut deduped = session_ids.clone();
    deduped.sort();
    deduped.dedup();
    assert_eq!(
        deduped.len(),
        session_ids.len(),
        "all sessions should have unique IDs"
    );
}

#[tokio::test]
async fn ws_proto_disconnect_during_init() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("disconnect-test");
    std::fs::create_dir_all(&ws_path).unwrap();

    // Phase A: prepare a run via POST, then immediately drop the
    // pending WS attachment without consuming it. The matching slot
    // sits in `RouterState::pending_chat_runs` until a stop or the
    // next POST consumes it.
    let payload = chat_request_payload(&ws_path, None);
    let resp = post_runtime_request(&server, &payload).await;
    assert!(resp.status().is_success());

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Verify server is still healthy
    let resp = http_client()
        .get(format!("{}/health", server.base_url()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn ws_proto_rapid_init_then_message() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("rapid-init-msg");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = open_chat_run(&server, &chat_request_payload(&ws_path, None)).await;

    // Send message immediately after init
    ws.send_user_message("quick message").await;

    // Should get a valid response (mock provider)
    let msg = ws.recv_json_timeout(Duration::from_secs(30)).await;
    assert!(
        msg.is_some(),
        "should receive response after rapid init+message"
    );
}
