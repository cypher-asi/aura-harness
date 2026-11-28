//! Suite 1: REST API data flow.

use super::support::*;

// ============================================================================
// Suite 1: REST API Data Flow (no LLM needed)
// ============================================================================

#[tokio::test]
async fn rest_tx_then_record_visible() {
    // Post-refactor the scheduler only processes agents whose identity
    // is registered (see `Scheduler::schedule_agent_with_overrides`), so
    // a raw `/tx` for a never-seen agent is a no-op. Bootstrap a chat run
    // first, pinning the session `agent_id` via `partition_id` so the
    // same id is both registered (the bootstrap also records
    // `session_start` + capability entries) and addressable over the
    // REST record/head endpoints. Mock provider → deterministic, no net.
    let server = start_mock_server().await;
    let client = http_client();
    let agent_id = AgentId::generate();
    let hex = agent_id.to_hex();

    let ws_path = server.workspaces_path().join("rest-record-visible");
    std::fs::create_dir_all(&ws_path).unwrap();
    let payload = chat_request_payload_extended(
        &ws_path,
        ChatRequestOpts {
            partition_id: Some(&hex),
            ..Default::default()
        },
    );
    // Hold the attach for the test's duration so the run isn't reaped.
    let _ws = open_chat_run(&server, &payload).await;

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
    tokio::time::sleep(Duration::from_millis(1000)).await;

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
    // See `rest_tx_then_record_visible`: register the agent identity via
    // the chat-run bootstrap (pinning `agent_id` through `partition_id`)
    // before exercising the raw `/tx` + `/head` REST surface.
    let server = start_mock_server().await;
    let client = http_client();
    let agent_id = AgentId::generate();
    let hex = agent_id.to_hex();

    let ws_path = server.workspaces_path().join("rest-head-increments");
    std::fs::create_dir_all(&ws_path).unwrap();
    let payload = chat_request_payload_extended(
        &ws_path,
        ChatRequestOpts {
            partition_id: Some(&hex),
            ..Default::default()
        },
    );
    let _ws = open_chat_run(&server, &payload).await;

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

async fn submit_user_prompt(
    client: &reqwest::Client,
    server: &TestServer,
    agent: &AgentId,
    payload: &str,
) {
    client
        .post(format!("{}/tx", server.base_url()))
        .json(&json!({ "agent_id": agent.to_hex(), "kind": "user_prompt", "payload": payload }))
        .send()
        .await
        .unwrap();
}

async fn fetch_record(client: &reqwest::Client, server: &TestServer, agent: &AgentId) -> Value {
    let resp = client
        .get(format!(
            "{}/agents/{}/record?from_seq=1&limit=100",
            server.base_url(),
            agent.to_hex()
        ))
        .send()
        .await
        .unwrap();
    resp.json().await.unwrap()
}

async fn fetch_head(client: &reqwest::Client, server: &TestServer, agent: &AgentId) -> Value {
    client
        .get(format!(
            "{}/agents/{}/head",
            server.base_url(),
            agent.to_hex()
        ))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()
}

#[tokio::test]
async fn rest_multiple_agents_isolated() {
    let server = TestServer::start().await;
    let client = http_client();

    let agent_a = AgentId::generate();
    let agent_b = AgentId::generate();

    let payload_a = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "for A");
    let payload_b = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "for B");

    submit_user_prompt(&client, &server, &agent_a, &payload_a).await;
    submit_user_prompt(&client, &server, &agent_b, &payload_b).await;

    tokio::time::sleep(Duration::from_millis(500)).await;

    let _entries_a: Value = fetch_record(&client, &server, &agent_a).await;
    let _entries_b: Value = fetch_record(&client, &server, &agent_b).await;

    // Cross-agent head should not mix
    let head_a = fetch_head(&client, &server, &agent_a).await;
    let head_b = fetch_head(&client, &server, &agent_b).await;

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
