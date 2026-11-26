//! Suite 8: concurrency and stress.

use crate::support::*;

// ============================================================================
// Suite 8: Concurrency & Stress
// ============================================================================

/// One independent session of `stress_parallel_sessions_independent`:
/// opens a chat run, asks for a uniquely named file and checks that the
/// model used `write_file`.
async fn run_parallel_session(
    i: i32,
    base_url: String,
    base_ws_url: String,
    ws_base: std::path::PathBuf,
    tok: String,
) -> (i32, std::path::PathBuf) {
    let ws_path = ws_base.join(format!("parallel-{i}"));
    std::fs::create_dir_all(&ws_path).unwrap();

    let tok_ref = if tok.is_empty() {
        None
    } else {
        Some(tok.as_str())
    };
    let payload = chat_request_payload(&ws_path, tok_ref);
    let client = http_client();
    let resp = client
        .post(format!("{base_url}/v1/run"))
        .json(&payload)
        .send()
        .await
        .expect("POST /v1/run");
    let body: Value = resp.json().await.expect("body");
    let run_id = body["run_id"].as_str().expect("run_id").to_string();
    let stream_url = format!("{base_ws_url}/stream/{run_id}");
    let mut ws = WsClient::connect_with_auth(&stream_url, common::E2E_TEST_BEARER).await;
    ws.expect_session_ready().await;

    let filename = format!("unique_{i}.txt");
    ws.send_user_message(&format!(
        "Use the write_file tool to create '{filename}' with content 'session {i}'. Do it now."
    ))
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"write_file".to_string()),
        "session {i}: expected write_file"
    );

    (i, ws_path)
}

#[tokio::test]
async fn stress_parallel_sessions_independent() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let base_url = server.base_url().to_string();
    let base_ws_url = server.base_ws_url().to_string();
    let ws_base = server.workspaces_path();

    let mut handles = Vec::new();
    for i in 0..3 {
        let base_url = base_url.clone();
        let base_ws_url = base_ws_url.clone();
        let ws_base = ws_base.clone();
        let tok = token.clone();

        handles.push(tokio::spawn(run_parallel_session(
            i,
            base_url,
            base_ws_url,
            ws_base,
            tok,
        )));
    }

    let results: Vec<_> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    // Verify each session created its own file without cross-contamination
    for (i, ws_path) in &results {
        let filename = format!("unique_{i}.txt");
        if let Some(path) = find_file(ws_path, &filename) {
            let content = std::fs::read_to_string(path).unwrap();
            assert!(
                content.contains(&format!("session {i}")),
                "session {i} file should contain its own content"
            );
        }
    }
}

#[tokio::test]
async fn stress_rapid_connect_disconnect() {
    let server = start_mock_server().await;

    // Phase A: spin up several pending chat runs without attaching
    // their WS to exercise the "client gives up before /stream/:run_id
    // upgrade" path. The `RouterState::pending_chat_runs` slots just
    // sit there until garbage collection / process exit.
    for i in 0..10 {
        let ws_path = server.workspaces_path().join(format!("rcd-{i}"));
        std::fs::create_dir_all(&ws_path).unwrap();
        let payload = chat_request_payload(&ws_path, None);
        let resp = post_runtime_request(&server, &payload).await;
        assert!(resp.status().is_success());
    }

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Server should still be healthy
    let resp = http_client()
        .get(format!("{}/health", server.base_url()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn stress_large_file_content() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("stress-large");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    // Pre-create a large file in the agent directory
    let large_content = "x".repeat(100_000); // 100KB
    place_file_in_agent_dir(&ws_path, "large.txt", &large_content);

    ws.send_user_message(
        "Use the stat_file tool to get metadata for 'large.txt'. Tell me its size.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"stat_file".to_string()),
        "expected stat_file on large file, got: {tools:?}"
    );
    assert_stop_reason(&messages, "end_turn");
}

#[tokio::test]
async fn stress_many_ws_sessions_health() {
    let server = start_mock_server().await;

    // Open several sessions and verify they all init successfully
    let mut sessions = Vec::new();
    for i in 0..5 {
        let ws_path = server.workspaces_path().join(format!("stress-multi-{i}"));
        std::fs::create_dir_all(&ws_path).unwrap();

        let payload = chat_request_payload(&ws_path, None);
        let resp = post_runtime_request(&server, &payload).await;
        let body: Value = resp.json().await.unwrap();
        let run_id = body["run_id"].as_str().unwrap().to_string();
        let mut ws = WsClient::connect(&server.stream_url(&run_id)).await;
        let ready = ws.expect_session_ready().await;
        assert!(
            ready["session_id"].is_string(),
            "session {i} should get session_id"
        );
        sessions.push(ws);
    }

    // All should still be alive -- server is not overwhelmed
    let resp = http_client()
        .get(format!("{}/health", server.base_url()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn stress_concurrent_rest_and_ws() {
    let server = TestServer::start().await;
    let client = http_client();

    // Verify REST works
    let agent_id = AgentId::generate();
    let payload_b64 = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        "concurrent test",
    );
    let body =
        json!({ "agent_id": agent_id.to_hex(), "kind": "user_prompt", "payload": payload_b64 });
    let resp = client
        .post(format!("{}/tx", server.base_url()))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202);

    // Simultaneously open a WS session
    let ws_path = server.workspaces_path().join("stress-rest-ws");
    std::fs::create_dir_all(&ws_path).unwrap();
    let mut ws = open_chat_run(&server, &chat_request_payload(&ws_path, None)).await;
    let _ = &mut ws; // keep ws alive

    // Verify REST still works while WS is open
    let resp = client
        .get(format!("{}/health", server.base_url()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}
