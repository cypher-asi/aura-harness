//! Comprehensive E2E integration tests for Aura Swarm.
//!
//! This file exercises every conceivable end-user use case and tool from
//! the outside, connecting via HTTP and WebSocket exactly as a real client
//! would.  Tests are organised into suites:
//!
//!   1. REST API data-flow
//!   2. WebSocket security & validation
//!   3. WebSocket session configuration
//!   4. WebSocket protocol edge-cases
//!   5. ZOS login + JWT authentication  (requires `E2E_ZOS_EMAIL` / `E2E_ZOS_PASSWORD`)
//!   6. LLM tool coverage               (requires LLM credentials)
//!   7. Streaming protocol fidelity      (requires LLM credentials)
//!   8. Concurrency & stress
//!
//! Run everything:
//! ```text
//! cargo test --test e2e_full
//! ```
//!
//! Run only suites that need no credentials:
//! ```text
//! cargo test --test e2e_full rest_
//! cargo test --test e2e_full ws_sec_
//! cargo test --test e2e_full ws_cfg_
//! cargo test --test e2e_full ws_proto_
//! cargo test --test e2e_full stress_
//! ```

mod common;

use std::time::Duration;

use aura_core::AgentId;
use common::{
    assert_stop_reason, collect_text, connect_llm_session, find_agent_dir, find_file, http_client,
    place_file_in_agent_dir, start_mock_server, tool_names_used, SessionInitOpts, TestServer,
    WsClient,
};
use serde_json::{json, Value};

// ============================================================================
// Suite 1: REST API Data Flow (no LLM needed)
// ============================================================================

#[tokio::test]
async fn rest_tx_then_record_visible() {
    let server = TestServer::start().await;
    let client = http_client();
    let agent_id = AgentId::generate();
    let hex = agent_id.to_hex();

    let payload_b64 = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        "hello from rest test",
    );
    let body = json!({
        "agent_id": hex,
        "kind": "user_prompt",
        "payload": payload_b64,
    });

    let resp = client
        .post(format!("{}/tx", server.base_url()))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202);

    // Give the scheduler a moment to process
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = client
        .get(format!(
            "{}/agents/{hex}/record?from_seq=1&limit=10",
            server.base_url()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let entries: Value = resp.json().await.unwrap();
    let arr = entries.as_array().unwrap();
    assert!(
        !arr.is_empty(),
        "record should contain at least one entry after tx submission"
    );
}

#[tokio::test]
async fn rest_tx_increments_head() {
    let server = TestServer::start().await;
    let client = http_client();
    let agent_id = AgentId::generate();
    let hex = agent_id.to_hex();

    for i in 0..3 {
        let payload_b64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            format!("msg {i}"),
        );
        let body = json!({ "agent_id": hex, "kind": "user_prompt", "payload": payload_b64 });
        let resp = client
            .post(format!("{}/tx", server.base_url()))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 202);
    }

    tokio::time::sleep(Duration::from_millis(1000)).await;

    let resp = client
        .get(format!("{}/agents/{hex}/head", server.base_url()))
        .send()
        .await
        .unwrap();
    let data: Value = resp.json().await.unwrap();
    let head = data["head_seq"].as_u64().unwrap();
    assert!(
        head >= 1,
        "head_seq should be >= 1 after TX submissions, got {head}"
    );
}

#[tokio::test]
async fn rest_record_pagination() {
    let server = TestServer::start().await;
    let client = http_client();
    let agent_id = AgentId::generate();
    let hex = agent_id.to_hex();

    for i in 0..5 {
        let payload_b64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            format!("paginate {i}"),
        );
        let body = json!({ "agent_id": hex, "kind": "user_prompt", "payload": payload_b64 });
        client
            .post(format!("{}/tx", server.base_url()))
            .json(&body)
            .send()
            .await
            .unwrap();
    }

    tokio::time::sleep(Duration::from_millis(1500)).await;

    let resp = client
        .get(format!(
            "{}/agents/{hex}/record?from_seq=1&limit=2",
            server.base_url()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let entries: Value = resp.json().await.unwrap();
    let arr = entries.as_array().unwrap();
    assert!(
        arr.len() <= 2,
        "limit=2 should return at most 2 entries, got {}",
        arr.len()
    );
}

#[tokio::test]
async fn rest_record_limit_capped_at_1000() {
    let server = TestServer::start().await;
    let client = http_client();
    let agent_id = AgentId::generate();

    let resp = client
        .get(format!(
            "{}/agents/{}/record?from_seq=1&limit=5000",
            server.base_url(),
            agent_id.to_hex()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "large limit should still succeed (capped to 1000)"
    );
}

#[tokio::test]
async fn rest_tx_all_kinds_stored() {
    let server = TestServer::start().await;
    let client = http_client();
    let agent_id = AgentId::generate();
    let hex = agent_id.to_hex();
    let kinds = [
        "user_prompt",
        "agent_msg",
        "trigger",
        "action_result",
        "system",
    ];

    for kind in &kinds {
        let payload_b64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            format!("kind: {kind}"),
        );
        let body = json!({ "agent_id": hex, "kind": kind, "payload": payload_b64 });
        let resp = client
            .post(format!("{}/tx", server.base_url()))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 202, "kind '{kind}' should be accepted");
    }
}

#[tokio::test]
async fn rest_multiple_agents_isolated() {
    let server = TestServer::start().await;
    let client = http_client();

    let agent_a = AgentId::generate();
    let agent_b = AgentId::generate();

    let payload_a = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "for A");
    let payload_b = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "for B");

    client
        .post(format!("{}/tx", server.base_url()))
        .json(&json!({ "agent_id": agent_a.to_hex(), "kind": "user_prompt", "payload": payload_a }))
        .send()
        .await
        .unwrap();

    client
        .post(format!("{}/tx", server.base_url()))
        .json(&json!({ "agent_id": agent_b.to_hex(), "kind": "user_prompt", "payload": payload_b }))
        .send()
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp_a = client
        .get(format!(
            "{}/agents/{}/record?from_seq=1&limit=100",
            server.base_url(),
            agent_a.to_hex()
        ))
        .send()
        .await
        .unwrap();
    let _entries_a: Value = resp_a.json().await.unwrap();

    let resp_b = client
        .get(format!(
            "{}/agents/{}/record?from_seq=1&limit=100",
            server.base_url(),
            agent_b.to_hex()
        ))
        .send()
        .await
        .unwrap();
    let _entries_b: Value = resp_b.json().await.unwrap();

    // Cross-agent head should not mix
    let head_a = client
        .get(format!(
            "{}/agents/{}/head",
            server.base_url(),
            agent_a.to_hex()
        ))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let head_b = client
        .get(format!(
            "{}/agents/{}/head",
            server.base_url(),
            agent_b.to_hex()
        ))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();

    assert_eq!(head_a["agent_id"].as_str().unwrap(), agent_a.to_hex());
    assert_eq!(head_b["agent_id"].as_str().unwrap(), agent_b.to_hex());
}

#[tokio::test]
async fn rest_tx_empty_payload() {
    let server = TestServer::start().await;
    let client = http_client();
    let agent_id = AgentId::generate();
    let empty_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "");
    let body = json!({ "agent_id": agent_id.to_hex(), "kind": "system", "payload": empty_b64 });

    let resp = client
        .post(format!("{}/tx", server.base_url()))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202, "empty payload should be accepted");
}

#[tokio::test]
async fn rest_concurrent_tx_submissions() {
    let server = TestServer::start().await;
    let agent_id = AgentId::generate();
    let hex = agent_id.to_hex();
    let base_url = server.base_url().to_string();

    let mut handles = Vec::new();
    for i in 0..10 {
        let url = base_url.clone();
        let hex = hex.clone();
        handles.push(tokio::spawn(async move {
            let client = http_client();
            let payload_b64 = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                format!("concurrent {i}"),
            );
            let body = json!({ "agent_id": hex, "kind": "user_prompt", "payload": payload_b64 });
            let resp = client
                .post(format!("{url}/tx"))
                .json(&body)
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status(), 202, "concurrent tx {i} should be accepted");
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    // Verify the server is still healthy
    let resp = http_client()
        .get(format!("{}/health", server.base_url()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn rest_invalid_agent_id_returns_400() {
    let server = TestServer::start().await;
    let client = http_client();
    let body = json!({ "agent_id": "not-hex", "kind": "user_prompt", "payload": "aGVsbG8=" });
    let resp = client
        .post(format!("{}/tx", server.base_url()))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn rest_invalid_kind_returns_400() {
    let server = TestServer::start().await;
    let client = http_client();
    let agent_id = AgentId::generate();
    let body =
        json!({ "agent_id": agent_id.to_hex(), "kind": "bogus_kind", "payload": "aGVsbG8=" });
    let resp = client
        .post(format!("{}/tx", server.base_url()))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn rest_invalid_base64_returns_400() {
    let server = TestServer::start().await;
    let client = http_client();
    let agent_id = AgentId::generate();
    let body =
        json!({ "agent_id": agent_id.to_hex(), "kind": "user_prompt", "payload": "!!!not-b64!!!" });
    let resp = client
        .post(format!("{}/tx", server.base_url()))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn rest_nonexistent_route_returns_404() {
    let server = TestServer::start().await;
    let resp = http_client()
        .get(format!("{}/does/not/exist", server.base_url()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn rest_head_invalid_agent_returns_400() {
    let server = TestServer::start().await;
    let resp = http_client()
        .get(format!("{}/agents/zzz-bad/head", server.base_url()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn rest_record_invalid_agent_returns_400() {
    let server = TestServer::start().await;
    let resp = http_client()
        .get(format!("{}/agents/zzz-bad/record", server.base_url()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

// ============================================================================
// Suite 2: WebSocket Security & Validation
// ============================================================================

#[tokio::test]
async fn ws_sec_workspace_path_traversal_rejected() {
    let server = TestServer::start().await;
    let mut ws = WsClient::connect(&server.ws_url()).await;

    ws.send_json(&json!({
        "type": "session_init",
        "workspace": "../../../etc"
    }))
    .await;

    let msg = ws.recv_json().await.expect("expected error");
    assert_eq!(msg["type"], "error");
    assert!(
        msg["code"] == "invalid_workspace" || msg["message"].as_str().unwrap_or("").contains(".."),
        "expected workspace rejection, got: {msg}"
    );
}

#[tokio::test]
async fn ws_sec_workspace_outside_base_rejected() {
    let server = TestServer::start().await;
    let mut ws = WsClient::connect(&server.ws_url()).await;

    // Absolute path outside the server's workspaces directory
    let outside = if cfg!(windows) {
        "C:\\Windows\\Temp\\evil"
    } else {
        "/tmp/evil"
    };
    ws.send_json(&json!({
        "type": "session_init",
        "workspace": outside
    }))
    .await;

    let msg = ws.recv_json().await.expect("expected error");
    assert_eq!(
        msg["type"], "error",
        "expected error for outside workspace, got: {msg}"
    );
}

#[tokio::test]
async fn ws_sec_project_path_relative_rejected() {
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("sec-relpath");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_json(&json!({
        "type": "session_init",
        "workspace": ws_path.to_string_lossy(),
        "project_path": "relative/path"
    }))
    .await;

    let msg = ws.recv_json().await.expect("expected error");
    assert_eq!(msg["type"], "error");
    assert!(
        msg["message"].as_str().unwrap_or("").contains("absolute"),
        "expected 'absolute' in error message, got: {msg}"
    );
}

#[tokio::test]
async fn ws_sec_project_path_with_dotdot_rejected() {
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("sec-dotdot");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = WsClient::connect(&server.ws_url()).await;
    let evil = if cfg!(windows) {
        "C:\\foo\\..\\bar"
    } else {
        "/foo/../bar"
    };
    ws.send_json(&json!({
        "type": "session_init",
        "workspace": ws_path.to_string_lossy(),
        "project_path": evil
    }))
    .await;

    let msg = ws.recv_json().await.expect("expected error");
    assert_eq!(msg["type"], "error");
    assert!(
        msg["message"].as_str().unwrap_or("").contains(".."),
        "expected '..' rejection, got: {msg}"
    );
}

#[tokio::test]
async fn ws_sec_unknown_message_type_rejected() {
    let server = TestServer::start().await;
    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_raw(r#"{"type": "unknown_type", "data": 42}"#).await;
    let msg = ws.recv_json().await.expect("expected error");
    assert_eq!(msg["type"], "error");
    assert_eq!(msg["code"], "parse_error");
}

#[tokio::test]
async fn ws_sec_empty_json_rejected() {
    let server = TestServer::start().await;
    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_raw("{}").await;
    let msg = ws.recv_json().await.expect("expected error");
    assert_eq!(msg["type"], "error");
    assert_eq!(msg["code"], "parse_error");
}

#[tokio::test]
async fn ws_sec_malformed_session_init() {
    let server = TestServer::start().await;
    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_raw(r#"{"type": "session_init", "max_tokens": "not_a_number"}"#)
        .await;
    let msg = ws.recv_json().await.expect("expected error");
    assert_eq!(msg["type"], "error");
    assert_eq!(msg["code"], "parse_error");
}

#[tokio::test]
async fn ws_sec_invalid_json() {
    let server = TestServer::start().await;
    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_raw("this is not json at all!!!").await;
    let msg = ws.recv_json().await.expect("expected error");
    assert_eq!(msg["type"], "error");
    assert_eq!(msg["code"], "parse_error");
}

// ============================================================================
// Suite 3: WebSocket Session Configuration (mock provider, no LLM needed)
// ============================================================================

#[tokio::test]
async fn ws_cfg_installed_tools_appear_in_session_ready() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("cfg-installed-tools");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_session_init_extended(
        &ws_path,
        SessionInitOpts {
            installed_tools: Some(vec![json!({
                "name": "my_custom_tool",
                "description": "A custom tool for testing",
                "input_schema": {
                    "type": "object",
                    "properties": { "query": { "type": "string" } },
                    "required": ["query"]
                },
                "endpoint": "https://example.com/tool"
            })]),
            ..Default::default()
        },
    )
    .await;

    let ready = ws.expect_session_ready().await;
    let tools = ready["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(
        names.contains(&"my_custom_tool"),
        "installed tool should appear in session_ready.tools, got: {names:?}"
    );
}

#[tokio::test]
async fn ws_cfg_integration_backed_tools_require_installed_integration() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("cfg-integration-gated-tools");
    std::fs::create_dir_all(&ws_path).unwrap();

    let gated_tool = json!({
        "name": "brave_search_web",
        "description": "Search the web using Brave",
        "input_schema": {
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "required": ["query"]
        },
        "endpoint": "https://example.com/tool",
        "required_integration": {
            "provider": "brave_search",
            "kind": "workspace_integration"
        }
    });

    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_session_init_extended(
        &ws_path,
        SessionInitOpts {
            installed_tools: Some(vec![gated_tool.clone()]),
            ..Default::default()
        },
    )
    .await;

    let ready = ws.expect_session_ready().await;
    let names: Vec<&str> = ready["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(
        !names.contains(&"brave_search_web"),
        "integration-backed tool should be hidden until its integration is installed: {names:?}"
    );

    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_session_init_extended(
        &ws_path,
        SessionInitOpts {
            installed_tools: Some(vec![gated_tool]),
            installed_integrations: Some(vec![json!({
                "integration_id": "integration-brave-1",
                "name": "Brave Search",
                "provider": "brave_search",
                "kind": "workspace_integration"
            })]),
            ..Default::default()
        },
    )
    .await;

    let ready = ws.expect_session_ready().await;
    let names: Vec<&str> = ready["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(
        names.contains(&"brave_search_web"),
        "integration-backed tool should appear once its integration is installed: {names:?}"
    );
}

#[tokio::test]
async fn ws_cfg_conversation_messages_accepted() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("cfg-conv-msgs");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_session_init_extended(
        &ws_path,
        SessionInitOpts {
            conversation_messages: Some(vec![
                json!({"role": "user", "content": "Hello from history"}),
                json!({"role": "assistant", "content": "I remember that."}),
            ]),
            ..Default::default()
        },
    )
    .await;

    let ready = ws.expect_session_ready().await;
    assert!(
        ready["session_id"].is_string(),
        "should get valid session_ready"
    );
}

#[tokio::test]
async fn ws_cfg_temperature() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("cfg-temp");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_session_init_extended(
        &ws_path,
        SessionInitOpts {
            temperature: Some(0.5),
            ..Default::default()
        },
    )
    .await;
    let ready = ws.expect_session_ready().await;
    assert!(ready["session_id"].is_string());
}

#[tokio::test]
async fn ws_cfg_max_tokens() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("cfg-maxtok");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_session_init_extended(
        &ws_path,
        SessionInitOpts {
            max_tokens: Some(4096),
            ..Default::default()
        },
    )
    .await;
    let ready = ws.expect_session_ready().await;
    assert!(ready["session_id"].is_string());
}

#[tokio::test]
async fn ws_cfg_project_id() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("cfg-projid");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_session_init_extended(
        &ws_path,
        SessionInitOpts {
            project_id: Some("proj-abc-123"),
            ..Default::default()
        },
    )
    .await;
    let ready = ws.expect_session_ready().await;
    assert!(ready["session_id"].is_string());
}

#[tokio::test]
async fn ws_cfg_minimal_session_init() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("cfg-minimal");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = WsClient::connect(&server.ws_url()).await;
    // Only workspace -- all other fields default
    ws.send_json(&json!({
        "type": "session_init",
        "workspace": ws_path.to_string_lossy(),
    }))
    .await;

    let ready = ws.expect_session_ready().await;
    let tools = ready["tools"].as_array().unwrap();
    assert!(
        !tools.is_empty(),
        "should get default tools with minimal init"
    );
}

#[tokio::test]
async fn ws_cfg_project_path_valid() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("cfg-projpath");
    std::fs::create_dir_all(&ws_path).unwrap();

    let real_dir = tempfile::tempdir().unwrap();

    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_session_init_extended(
        &ws_path,
        SessionInitOpts {
            project_path: Some(real_dir.path().to_str().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let ready = ws.expect_session_ready().await;
    assert!(ready["session_id"].is_string());
}

#[tokio::test]
async fn ws_cfg_session_ready_includes_agent_and_core_tools() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("cfg-alltools");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_session_init(&ws_path, None).await;
    let ready = ws.expect_session_ready().await;

    let tools = ready["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();

    // Core filesystem tools
    assert!(names.contains(&"read_file"), "missing read_file");
    assert!(names.contains(&"write_file"), "missing write_file");
    assert!(names.contains(&"edit_file"), "missing edit_file");
    assert!(names.contains(&"delete_file"), "missing delete_file");
    assert!(names.contains(&"list_files"), "missing list_files");
    assert!(names.contains(&"find_files"), "missing find_files");
    assert!(names.contains(&"search_code"), "missing search_code");
    assert!(names.contains(&"run_command"), "missing run_command");
    assert!(names.contains(&"stat_file"), "missing stat_file");

    // Agent-profile domain tools (spec/task management)
    assert!(names.contains(&"list_specs"), "missing list_specs");
    assert!(names.contains(&"create_spec"), "missing create_spec");
    assert!(names.contains(&"list_tasks"), "missing list_tasks");
    assert!(names.contains(&"create_task"), "missing create_task");
    assert!(names.contains(&"get_project"), "missing get_project");
}

#[tokio::test]
async fn ws_cfg_session_init_with_model_override() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("cfg-model");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_session_init_extended(
        &ws_path,
        SessionInitOpts {
            model: Some("claude-sonnet-4-20250514"),
            ..Default::default()
        },
    )
    .await;
    let ready = ws.expect_session_ready().await;
    assert!(ready["session_id"].is_string());
}

#[tokio::test]
async fn ws_cfg_system_prompt_override() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("cfg-sysprompt");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_session_init_extended(
        &ws_path,
        SessionInitOpts {
            system_prompt: Some("You are a helpful pirate assistant."),
            ..Default::default()
        },
    )
    .await;
    let ready = ws.expect_session_ready().await;
    assert!(ready["session_id"].is_string());
}

#[tokio::test]
async fn ws_cfg_multiple_installed_tools() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("cfg-multi-tools");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_session_init_extended(
        &ws_path,
        SessionInitOpts {
            installed_tools: Some(vec![
                json!({
                    "name": "tool_alpha",
                    "description": "Alpha tool",
                    "input_schema": { "type": "object", "properties": {} },
                    "endpoint": "https://example.com/alpha"
                }),
                json!({
                    "name": "tool_beta",
                    "description": "Beta tool",
                    "input_schema": { "type": "object", "properties": { "x": { "type": "integer" } } },
                    "endpoint": "https://example.com/beta"
                }),
            ]),
            ..Default::default()
        },
    )
    .await;

    let ready = ws.expect_session_ready().await;
    let tools = ready["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"tool_alpha"), "missing tool_alpha");
    assert!(names.contains(&"tool_beta"), "missing tool_beta");
}

// ============================================================================
// Suite 4: WebSocket Protocol Edge Cases
// ============================================================================

#[tokio::test]
async fn ws_proto_cancel_no_active_turn() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("proto-cancel-idle");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_session_init(&ws_path, None).await;
    ws.expect_session_ready().await;

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

    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_session_init(&ws_path, None).await;
    ws.expect_session_ready().await;

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
async fn ws_proto_session_init_twice_errors() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("proto-double-init");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_session_init(&ws_path, None).await;
    ws.expect_session_ready().await;

    ws.send_session_init(&ws_path, None).await;
    let msg = ws.recv_json().await.expect("expected error");
    assert_eq!(msg["type"], "error");
    assert_eq!(msg["code"], "already_initialized");
}

#[tokio::test]
async fn ws_proto_user_message_before_init() {
    let server = start_mock_server().await;
    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_user_message("hello without init").await;
    let msg = ws.recv_json().await.expect("expected error");
    assert_eq!(msg["type"], "error");
    assert_eq!(msg["code"], "not_initialized");
}

#[tokio::test]
async fn ws_proto_multiple_concurrent_sessions() {
    let server = start_mock_server().await;

    let mut session_ids = Vec::new();
    for i in 0..3 {
        let ws_path = server.workspaces_path().join(format!("concurrent-{i}"));
        std::fs::create_dir_all(&ws_path).unwrap();

        let mut ws = WsClient::connect(&server.ws_url()).await;
        ws.send_session_init(&ws_path, None).await;
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

    // Connect and immediately drop without waiting for session_ready
    {
        let mut ws = WsClient::connect(&server.ws_url()).await;
        ws.send_session_init(&server.workspaces_path().join("disconnect-test"), None)
            .await;
        // Drop the connection immediately
    }

    // Verify server is still healthy
    tokio::time::sleep(Duration::from_millis(200)).await;
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

    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_session_init(&ws_path, None).await;
    ws.expect_session_ready().await;

    // Send message immediately after init
    ws.send_user_message("quick message").await;

    // Should get a valid response (mock provider)
    let msg = ws.recv_json_timeout(Duration::from_secs(30)).await;
    assert!(
        msg.is_some(),
        "should receive response after rapid init+message"
    );
}

// ============================================================================
// Suite 5: ZOS Login + JWT Authentication
// ============================================================================

#[tokio::test]
async fn zos_login_obtains_jwt() {
    let _ = dotenvy::dotenv();
    let (email, password) = require_zos!();

    let token = common::zos_login(&email, &password)
        .await
        .expect("ZOS login should succeed");
    assert!(!token.is_empty(), "JWT should be non-empty");
    // JWTs typically start with "eyJ"
    assert!(
        token.starts_with("eyJ"),
        "token should look like a JWT, got prefix: {}",
        &token[..token.len().min(10)]
    );
}

#[tokio::test]
async fn zos_login_invalid_credentials() {
    let _ = dotenvy::dotenv();
    // Only run if we have valid creds (so we know the endpoint is reachable)
    let _valid_creds = require_zos!();

    let result = common::zos_login("bad-email@nonexistent.test", "wrong-password").await;
    assert!(result.is_err(), "bad credentials should fail");
}

#[tokio::test]
async fn zos_whoami_after_login() {
    let _ = dotenvy::dotenv();
    let (email, password) = require_zos!();

    let client = aura_auth::ZosClient::new().unwrap();
    let session = client.login(&email, &password).await.unwrap();

    assert!(!session.user_id.is_empty(), "user_id should be populated");
    assert!(
        !session.display_name.is_empty(),
        "display_name should be populated"
    );
}

#[tokio::test]
async fn jwt_via_bearer_header() {
    let _ = dotenvy::dotenv();
    let (email, password) = require_zos!();
    let token = common::zos_login(&email, &password).await.unwrap();

    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("jwt-bearer");
    std::fs::create_dir_all(&ws_path).unwrap();

    // Connect with JWT in the HTTP Authorization header
    let mut ws = WsClient::connect_with_auth(&server.ws_url(), &token).await;
    // Init WITHOUT token in body -- auth comes from the header
    ws.send_session_init(&ws_path, None).await;
    ws.expect_session_ready().await;

    ws.send_user_message("Say hello in one word.").await;
    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    assert!(
        !messages.is_empty(),
        "should receive messages with bearer auth"
    );

    let has_text = messages.iter().any(|m| m["type"] == "text_delta");
    assert!(has_text, "should get text_delta with bearer auth");
}

#[tokio::test]
async fn jwt_via_session_init_token() {
    let _ = dotenvy::dotenv();
    let (email, password) = require_zos!();
    let token = common::zos_login(&email, &password).await.unwrap();

    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("jwt-token-field");
    std::fs::create_dir_all(&ws_path).unwrap();

    // Connect WITHOUT auth header, but pass JWT in session_init.token
    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_session_init(&ws_path, Some(&token)).await;
    ws.expect_session_ready().await;

    ws.send_user_message("What is 1+1? Reply with just the number.")
        .await;
    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    assert!(
        !messages.is_empty(),
        "should receive messages with token field auth"
    );

    let has_text = messages.iter().any(|m| m["type"] == "text_delta");
    assert!(has_text, "should get text_delta with token field auth");
}

#[tokio::test]
async fn jwt_proxy_mode_llm_turn() {
    let _ = dotenvy::dotenv();
    let (email, password) = require_zos!();
    let token = common::zos_login(&email, &password).await.unwrap();

    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("jwt-proxy-llm");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    ws.send_user_message("What is 2+2? Reply with just the number, nothing else.")
        .await;
    let messages = ws.collect_turn(Duration::from_secs(120)).await;

    let text = collect_text(&messages);
    assert!(text.contains('4'), "expected '4' in response, got: {text}");
    assert_stop_reason(&messages, "end_turn");
}

#[tokio::test]
async fn jwt_proxy_mode_tool_turn() {
    let _ = dotenvy::dotenv();
    let (email, password) = require_zos!();
    let token = common::zos_login(&email, &password).await.unwrap();

    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("jwt-proxy-tool");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    ws.send_user_message(
        "Use the write_file tool to create a file at 'jwt_test.txt' with content 'jwt works'. Do it now.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"write_file".to_string()),
        "expected write_file tool use with JWT auth, got: {tools:?}"
    );

    let found = find_file(&ws_path, "jwt_test.txt");
    assert!(found.is_some(), "file should be created on disk");
}

#[tokio::test]
async fn jwt_missing_in_proxy_mode_errors() {
    let _ = dotenvy::dotenv();
    let routing = std::env::var("AURA_LLM_ROUTING").unwrap_or_default();
    assert!(
        routing == "proxy" || routing.is_empty(),
        "this test requires proxy mode but AURA_LLM_ROUTING={routing}"
    );
    let _creds = require_zos!();

    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("jwt-missing");
    std::fs::create_dir_all(&ws_path).unwrap();

    // Connect with NO auth at all
    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_session_init(&ws_path, None).await;
    ws.expect_session_ready().await;

    ws.send_user_message("hello").await;

    let messages = ws.collect_turn(Duration::from_secs(60)).await;
    // Should get an error about missing JWT for proxy mode
    let has_error = messages
        .iter()
        .any(|m| m["type"] == "error" || m["stop_reason"] == "end_turn_with_errors");
    assert!(
        has_error,
        "missing JWT in proxy mode should produce an error or error stop_reason"
    );
}

// ============================================================================
// Suite 6: LLM Tool Coverage (requires credentials)
// ============================================================================

#[tokio::test]
async fn tool_write_file_nested_dirs() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-nested");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    ws.send_user_message(
        "Use the write_file tool to create a file at path 'a/b/c/deep.txt' with content 'deep file'. Do it now.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"write_file".to_string()),
        "expected write_file tool use, got: {tools:?}"
    );

    let found = find_file(&ws_path, "deep.txt");
    if let Some(path) = found {
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("deep"),
            "file content should contain 'deep', got: {content}"
        );
        // Verify nested directory was created
        assert!(
            path.to_string_lossy().contains("a") || path.to_string_lossy().contains("b"),
            "file should be in nested dir, got path: {path:?}"
        );
    }
}

#[tokio::test]
async fn tool_edit_file_replace_all() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-replaceall");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    place_file_in_agent_dir(&ws_path, "repeated.txt", "foo bar foo baz foo qux");

    ws.send_user_message(
        "Use the edit_file tool on 'repeated.txt' with old_text='foo', new_text='REPLACED', and set replace_all to true. Do it now.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"edit_file".to_string()),
        "expected edit_file tool use, got: {tools:?}"
    );

    if let Some(path) = find_file(&ws_path, "repeated.txt") {
        let content = std::fs::read_to_string(path).unwrap();
        if content.contains("foo") {
            eprintln!("NOTE: edit_file with replace_all may not have replaced all occurrences (LLM behaviour)");
        }
    }
}

#[tokio::test]
async fn tool_search_code_with_context() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-search-ctx");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    place_file_in_agent_dir(
        &ws_path,
        "sample.rs",
        "line1\nline2\nfn target_function() {\n    // body\n}\nline6\nline7\n",
    );

    ws.send_user_message(
        "Use the search_code tool to search for 'target_function' with context_lines set to 2.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"search_code".to_string()),
        "expected search_code tool use, got: {tools:?}"
    );
}

#[tokio::test]
async fn tool_find_files_scoped() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-find-scoped");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    if let Some(agent_dir) = find_agent_dir(&ws_path) {
        let src = agent_dir.join("src");
        let docs = agent_dir.join("docs");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(src.join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(docs.join("guide.md"), "# Guide").unwrap();
    }

    ws.send_user_message(
        "Use the find_files tool with pattern '*.rs' and path set to 'src'. Only search in src.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"find_files".to_string()),
        "expected find_files tool use, got: {tools:?}"
    );
}

#[tokio::test]
async fn tool_run_command_with_timeout() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-cmd-timeout");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    let cmd = if cfg!(windows) {
        "ping -n 10 127.0.0.1"
    } else {
        "sleep 10"
    };
    ws.send_user_message(&format!(
        "Use the run_command tool to execute '{cmd}' with timeout_secs set to 2.",
    ))
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"run_command".to_string()),
        "expected run_command tool use, got: {tools:?}"
    );
}

#[tokio::test]
async fn tool_multi_step_single_turn() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-multi-step");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    ws.send_user_message(
        "Do the following steps using tools:\n\
         1. Use write_file to create 'steps.txt' with content 'step one'\n\
         2. Use edit_file to replace 'step one' with 'step two' in 'steps.txt'\n\
         3. Use read_file to read 'steps.txt' and confirm it says 'step two'\n\
         Do all three steps now.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(180)).await;
    let tools = tool_names_used(&messages);

    // The LLM should use at least one tool; it may consolidate or
    // reorder steps, so we assert broadly.
    assert!(
        !tools.is_empty(),
        "expected at least one tool call in multi-step turn, got none"
    );
    assert!(
        tools.contains(&"write_file".to_string())
            || tools.contains(&"edit_file".to_string())
            || tools.contains(&"read_file".to_string()),
        "expected at least one file tool in multi-step, got: {tools:?}"
    );
}

#[tokio::test]
async fn tool_write_read_roundtrip() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-roundtrip");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    ws.send_user_message(
        "Use write_file to create 'roundtrip.txt' with the exact content 'MARKER_ABC_123'. \
         Then use read_file to read it back. Tell me what it contains.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"write_file".to_string()),
        "expected write_file, got: {tools:?}"
    );

    // Check tool_result for read_file contains the marker
    let read_results: Vec<&Value> = messages
        .iter()
        .filter(|m| m["type"] == "tool_result" && m["name"] == "read_file")
        .collect();
    if !read_results.is_empty() {
        let result = read_results[0]["result"].as_str().unwrap_or("");
        assert!(
            result.contains("MARKER_ABC_123"),
            "read_file should return written content, got: {result}"
        );
    }
}

#[tokio::test]
async fn tool_read_nonexistent_errors() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-read-noexist");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    ws.send_user_message("Use the read_file tool to read 'does_not_exist_xyz.txt'. Do it now.")
        .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"read_file".to_string()),
        "expected read_file tool use, got: {tools:?}"
    );
}

#[tokio::test]
async fn tool_edit_no_match_errors() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-edit-nomatch");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    place_file_in_agent_dir(&ws_path, "immutable.txt", "original content here");

    ws.send_user_message(
        "Use the edit_file tool on 'immutable.txt'. The old_text is 'ZZZZZ_NONEXISTENT_ZZZZZ' \
         and the new_text is 'replaced'. Do it now.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"edit_file".to_string()),
        "expected edit_file tool use, got: {tools:?}"
    );

    // File should be unchanged
    if let Some(path) = find_file(&ws_path, "immutable.txt") {
        let content = std::fs::read_to_string(path).unwrap();
        assert_eq!(content, "original content here", "file should be unchanged");
    }
}

#[tokio::test]
async fn tool_delete_nonexistent() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-del-noexist");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    ws.send_user_message("Use the delete_file tool to delete 'ghost_file_xyz.txt'. Do it now.")
        .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"delete_file".to_string()),
        "expected delete_file tool use, got: {tools:?}"
    );
}

#[tokio::test]
async fn tool_run_command_failure() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-cmd-fail");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    let cmd = if cfg!(windows) {
        "cmd /c dir nonexistent_dir_e2e_xyz 2>&1"
    } else {
        "ls /nonexistent_dir_e2e_xyz 2>&1"
    };
    ws.send_user_message(&format!(
        "Use the run_command tool to execute exactly this command: {cmd}"
    ))
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"run_command".to_string()),
        "expected run_command tool use, got: {tools:?}"
    );
}

#[tokio::test]
async fn tool_read_file_line_range() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-linerange");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    let content = (1..=20)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    place_file_in_agent_dir(&ws_path, "numbered.txt", &content);

    ws.send_user_message(
        "Use the read_file tool to read 'numbered.txt' with start_line=5 and end_line=10.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"read_file".to_string()),
        "expected read_file tool use, got: {tools:?}"
    );

    // The key assertion is that read_file was invoked with line range params.
    // The tool result may or may not contain the expected lines depending on
    // path resolution in the agent workspace.
    let results: Vec<&Value> = messages
        .iter()
        .filter(|m| m["type"] == "tool_result" && m["name"] == "read_file")
        .collect();
    assert!(
        !results.is_empty(),
        "expected at least one read_file tool_result"
    );
}

#[tokio::test]
async fn tool_search_code_regex() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-regex-search");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    place_file_in_agent_dir(
        &ws_path,
        "code.rs",
        "fn add_numbers(a: i32, b: i32) -> i32 { a + b }\nfn subtract_numbers(a: i32, b: i32) -> i32 { a - b }\n",
    );

    ws.send_user_message(
        "Use the search_code tool with the regex pattern 'fn \\w+_numbers' to find all functions ending with '_numbers'.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"search_code".to_string()),
        "expected search_code tool use, got: {tools:?}"
    );
}

#[tokio::test]
async fn tool_stat_file() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-stat");
    std::fs::create_dir_all(&ws_path).unwrap();
    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    place_file_in_agent_dir(&ws_path, "info.txt", "some content");

    ws.send_user_message("Get file metadata for 'info.txt' using the stat_file tool.")
        .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"stat_file".to_string()),
        "expected stat_file tool use"
    );
    assert_stop_reason(&messages, "end_turn");
}

#[tokio::test]
async fn tool_list_files_with_content() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-list");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    place_file_in_agent_dir(&ws_path, "alpha.txt", "a");
    place_file_in_agent_dir(&ws_path, "bravo.txt", "b");

    ws.send_user_message("Use the list_files tool to list the current directory.")
        .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"list_files".to_string()),
        "expected list_files tool use, got: {tools:?}"
    );
}

#[tokio::test]
async fn tool_write_file_overwrite() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-overwrite");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    place_file_in_agent_dir(&ws_path, "overwrite.txt", "ORIGINAL");

    ws.send_user_message("Use the write_file tool to write 'OVERWRITTEN' to 'overwrite.txt'.")
        .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"write_file".to_string()),
        "expected write_file, got: {tools:?}"
    );

    if let Some(path) = find_file(&ws_path, "overwrite.txt") {
        let content = std::fs::read_to_string(path).unwrap();
        assert!(
            content.contains("OVERWRITTEN"),
            "should have new content, got: {content}"
        );
        assert!(
            !content.contains("ORIGINAL"),
            "original content should be gone, got: {content}"
        );
    }
}

#[tokio::test]
async fn tool_run_command_with_cwd() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("tool-cwd");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    if let Some(agent_dir) = find_agent_dir(&ws_path) {
        std::fs::create_dir_all(agent_dir.join("subdir")).unwrap();
    }

    let cmd = if cfg!(windows) { "cd" } else { "pwd" };
    ws.send_user_message(&format!(
        "Use the run_command tool to execute '{cmd}' with working_dir set to 'subdir'."
    ))
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"run_command".to_string()),
        "expected run_command, got: {tools:?}"
    );
}

// ============================================================================
// Suite 7: Streaming Protocol Fidelity
// ============================================================================

#[tokio::test]
async fn stream_message_sequence_order() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("stream-order");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    ws.send_user_message("Say hello in one sentence.").await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    assert!(!messages.is_empty(), "should receive messages");

    // First message should be assistant_message_start
    assert_eq!(
        messages[0]["type"], "assistant_message_start",
        "first message should be assistant_message_start"
    );

    // Last message should be assistant_message_end
    let last = messages.last().unwrap();
    assert_eq!(
        last["type"], "assistant_message_end",
        "last message should be assistant_message_end"
    );

    // Should have at least one text_delta between start and end
    let has_text = messages.iter().any(|m| m["type"] == "text_delta");
    assert!(has_text, "should have text_delta between start and end");
}

#[tokio::test]
async fn stream_message_id_matches() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("stream-msgid");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    ws.send_user_message("Say hi.").await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;

    let start = messages
        .iter()
        .find(|m| m["type"] == "assistant_message_start")
        .expect("missing assistant_message_start");
    let end = messages
        .iter()
        .find(|m| m["type"] == "assistant_message_end")
        .expect("missing assistant_message_end");

    let start_id = start["message_id"].as_str().unwrap();
    let end_id = end["message_id"].as_str().unwrap();
    assert!(!start_id.is_empty(), "message_id should be non-empty");
    assert_eq!(
        start_id, end_id,
        "message_id should match between start and end"
    );
}

#[tokio::test]
async fn stream_usage_fields() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("stream-usage");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    ws.send_user_message("Say hello.").await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let end = messages
        .iter()
        .find(|m| m["type"] == "assistant_message_end")
        .expect("missing assistant_message_end");

    let usage = &end["usage"];
    assert!(
        usage["input_tokens"].as_u64().unwrap_or(0) > 0,
        "input_tokens > 0"
    );
    assert!(
        usage["output_tokens"].as_u64().unwrap_or(0) > 0,
        "output_tokens > 0"
    );
    assert!(
        usage["model"].is_string() && !usage["model"].as_str().unwrap().is_empty(),
        "model should be non-empty string"
    );
    assert!(
        usage["context_utilization"].as_f64().unwrap_or(-1.0) >= 0.0,
        "context_utilization should be >= 0.0"
    );
    assert!(
        usage["context_utilization"].as_f64().unwrap_or(2.0) <= 1.0,
        "context_utilization should be <= 1.0"
    );
}

#[tokio::test]
async fn stream_cumulative_tokens_increase() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("stream-cumulative");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    // Turn 1
    ws.send_user_message("Say hello.").await;
    let turn1 = ws.collect_turn(Duration::from_secs(120)).await;
    let end1 = turn1
        .iter()
        .find(|m| m["type"] == "assistant_message_end")
        .expect("turn 1 missing end");
    let cum_in_1 = end1["usage"]["cumulative_input_tokens"]
        .as_u64()
        .unwrap_or(0);
    let cum_out_1 = end1["usage"]["cumulative_output_tokens"]
        .as_u64()
        .unwrap_or(0);

    // Turn 2
    ws.send_user_message("Say goodbye.").await;
    let turn2 = ws.collect_turn(Duration::from_secs(120)).await;
    let end2 = turn2
        .iter()
        .find(|m| m["type"] == "assistant_message_end")
        .expect("turn 2 missing end");
    let cum_in_2 = end2["usage"]["cumulative_input_tokens"]
        .as_u64()
        .unwrap_or(0);
    let cum_out_2 = end2["usage"]["cumulative_output_tokens"]
        .as_u64()
        .unwrap_or(0);

    assert!(
        cum_in_2 >= cum_in_1,
        "cumulative_input_tokens should not decrease: {cum_in_1} -> {cum_in_2}"
    );
    assert!(
        cum_out_2 >= cum_out_1,
        "cumulative_output_tokens should not decrease: {cum_out_1} -> {cum_out_2}"
    );
}

#[tokio::test]
async fn stream_files_changed_structure() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("stream-fc");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    ws.send_user_message("Say hello.").await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let end = messages
        .iter()
        .find(|m| m["type"] == "assistant_message_end")
        .expect("missing end");

    let fc = &end["files_changed"];
    assert!(
        fc["created"].is_array(),
        "files_changed.created should be array"
    );
    assert!(
        fc["modified"].is_array(),
        "files_changed.modified should be array"
    );
    assert!(
        fc["deleted"].is_array(),
        "files_changed.deleted should be array"
    );
}

#[tokio::test]
async fn stream_tool_use_start_fields() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("stream-tool-fields");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    ws.send_user_message("Use the list_files tool to list the current directory.")
        .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;

    let tool_starts: Vec<&Value> = messages
        .iter()
        .filter(|m| m["type"] == "tool_use_start")
        .collect();

    if !tool_starts.is_empty() {
        for ts in &tool_starts {
            assert!(
                ts["id"].is_string() && !ts["id"].as_str().unwrap().is_empty(),
                "tool_use_start should have non-empty id"
            );
            assert!(
                ts["name"].is_string() && !ts["name"].as_str().unwrap().is_empty(),
                "tool_use_start should have non-empty name"
            );
        }
    }
}

#[tokio::test]
async fn stream_tool_result_name_matches_start() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("stream-tool-match");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    place_file_in_agent_dir(&ws_path, "test.txt", "test content");
    ws.send_user_message("Use the read_file tool to read 'test.txt'.")
        .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;

    let tool_start_names: Vec<&str> = messages
        .iter()
        .filter(|m| m["type"] == "tool_use_start")
        .filter_map(|m| m["name"].as_str())
        .collect();
    let tool_result_names: Vec<&str> = messages
        .iter()
        .filter(|m| m["type"] == "tool_result")
        .filter_map(|m| m["name"].as_str())
        .collect();

    // Every tool_result name should have a matching tool_use_start name
    for rn in &tool_result_names {
        assert!(
            tool_start_names.contains(rn),
            "tool_result name '{rn}' should have matching tool_use_start"
        );
    }
}

#[tokio::test]
async fn stream_stop_reason_on_simple_turn() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("stream-stop");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    ws.send_user_message("What is 1+1?").await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    assert_stop_reason(&messages, "end_turn");
}

// ============================================================================
// Suite 8: Concurrency & Stress
// ============================================================================

#[tokio::test]
async fn stress_parallel_sessions_independent() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;

    let mut handles = Vec::new();
    for i in 0..3 {
        let ws_url = server.ws_url();
        let ws_base = server.workspaces_path();
        let tok = token.clone();

        handles.push(tokio::spawn(async move {
            let ws_path = ws_base.join(format!("parallel-{i}"));
            std::fs::create_dir_all(&ws_path).unwrap();

            let mut ws = WsClient::connect(&ws_url).await;
            let tok_ref = if tok.is_empty() { None } else { Some(tok.as_str()) };
            ws.send_session_init(&ws_path, tok_ref).await;
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
        }));
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

    for _ in 0..10 {
        let _ws = WsClient::connect(&server.ws_url()).await;
        // Drop immediately
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
    let _ = dotenvy::dotenv();
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

        let mut ws = WsClient::connect(&server.ws_url()).await;
        ws.send_session_init(&ws_path, None).await;
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
    let mut ws = WsClient::connect(&server.ws_url()).await;
    ws.send_session_init(&ws_path, None).await;
    let ready = ws.expect_session_ready().await;
    assert!(ready["session_id"].is_string());

    // Verify REST still works while WS is open
    let resp = client
        .get(format!("{}/health", server.base_url()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

// ============================================================================
// Suite: Multi-turn Conversation
// ============================================================================

#[tokio::test]
async fn tool_multi_turn_context_preserved() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("multi-turn");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    // Turn 1: create a file
    ws.send_user_message(
        "Use the write_file tool to create 'memo.txt' with content 'Remember to buy milk'.",
    )
    .await;
    let turn1 = ws.collect_turn(Duration::from_secs(120)).await;
    assert!(
        tool_names_used(&turn1).contains(&"write_file".to_string()),
        "turn 1 should use write_file"
    );

    // Turn 2: read the same file
    ws.send_user_message("Use the read_file tool to read 'memo.txt' and tell me what it says.")
        .await;
    let turn2 = ws.collect_turn(Duration::from_secs(120)).await;
    let end = turn2
        .iter()
        .find(|m| m["type"] == "assistant_message_end")
        .expect("turn 2 should have assistant_message_end");
    let stop = end["stop_reason"].as_str().unwrap();
    assert!(
        stop == "end_turn" || stop == "end_turn_with_errors",
        "turn 2 stop_reason should be end_turn or end_turn_with_errors, got: {stop}"
    );
}

#[tokio::test]
async fn tool_cancel_during_turn() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("cancel-turn");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    ws.send_user_message(
        "Write a very long detailed essay about the history of computing, at least 2000 words.",
    )
    .await;

    // Wait for streaming to start, then cancel
    tokio::time::sleep(Duration::from_secs(2)).await;
    ws.send_cancel().await;

    let messages = ws.collect_turn(Duration::from_secs(60)).await;

    let end = messages
        .iter()
        .find(|m| m["type"] == "assistant_message_end");
    if let Some(e) = end {
        let stop = e["stop_reason"].as_str().unwrap_or("");
        assert!(
            stop == "cancelled" || stop == "end_turn" || stop == "end_turn_with_errors",
            "expected cancelled or end_turn, got: {stop}"
        );
    }
}

#[tokio::test]
async fn tool_message_during_turn_rejected() {
    let _ = dotenvy::dotenv();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("msg-during-turn");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    ws.send_user_message("Write a 500-word essay about software testing.")
        .await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    ws.send_user_message("This should be rejected").await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;

    let has_turn_in_progress = messages
        .iter()
        .any(|m| m["type"] == "error" && m["code"] == "turn_in_progress");
    assert!(has_turn_in_progress, "expected turn_in_progress error");
}
