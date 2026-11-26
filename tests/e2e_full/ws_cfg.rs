//! Suite 3: WebSocket session configuration.

use crate::support::*;

// ============================================================================
// Suite 3: WebSocket Session Configuration (mock provider, no LLM needed)
// ============================================================================

#[tokio::test]
async fn ws_cfg_installed_tools_appear_in_session_ready() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("cfg-installed-tools");
    std::fs::create_dir_all(&ws_path).unwrap();

    let payload = chat_request_payload_extended(
        &ws_path,
        ChatRequestOpts {
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
    );
    let resp = post_runtime_request(&server, &payload).await;
    assert_eq!(resp.status(), 201);
    let body: Value = resp.json().await.unwrap();
    let run_id = body["run_id"].as_str().unwrap().to_string();

    let stream_url = server.stream_url(&run_id);
    let mut ws = WsClient::connect(&stream_url).await;
    let ready = ws.expect_session_ready().await;

    let tools = ready["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(
        names.contains(&"my_custom_tool"),
        "installed tool should appear in session_ready.tools, got: {names:?}"
    );
}

/// Opens a chat run for `payload` and returns the tool names announced in
/// its `session_ready` frame.
async fn session_ready_tool_names(server: &TestServer, payload: &Value) -> Vec<String> {
    let resp = post_runtime_request(server, payload).await;
    let body: Value = resp.json().await.unwrap();
    let run_id = body["run_id"].as_str().unwrap().to_string();
    let mut ws = WsClient::connect(&server.stream_url(&run_id)).await;
    let ready = ws.expect_session_ready().await;
    ready["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .map(str::to_string)
        .collect()
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

    let payload_without_integration = chat_request_payload_extended(
        &ws_path,
        ChatRequestOpts {
            installed_tools: Some(vec![gated_tool.clone()]),
            ..Default::default()
        },
    );
    let names = session_ready_tool_names(&server, &payload_without_integration).await;
    assert!(
        !names.iter().any(|name| name == "brave_search_web"),
        "integration-backed tool should be hidden until its integration is installed: {names:?}"
    );

    let payload_with_integration = chat_request_payload_extended(
        &ws_path,
        ChatRequestOpts {
            installed_tools: Some(vec![gated_tool]),
            installed_integrations: Some(vec![json!({
                "integration_id": "integration-brave-1",
                "name": "Brave Search",
                "provider": "brave_search",
                "kind": "workspace_integration"
            })]),
            ..Default::default()
        },
    );
    let names = session_ready_tool_names(&server, &payload_with_integration).await;
    assert!(
        names.iter().any(|name| name == "brave_search_web"),
        "integration-backed tool should appear once its integration is installed: {names:?}"
    );
}

#[tokio::test]
async fn ws_cfg_conversation_messages_accepted() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("cfg-conv-msgs");
    std::fs::create_dir_all(&ws_path).unwrap();

    let payload = chat_request_payload_extended(
        &ws_path,
        ChatRequestOpts {
            conversation_messages: Some(vec![
                json!({"role": "user", "content": "Hello from history"}),
                json!({"role": "assistant", "content": "I remember that."}),
            ]),
            ..Default::default()
        },
    );
    let resp = post_runtime_request(&server, &payload).await;
    let body: Value = resp.json().await.unwrap();
    let run_id = body["run_id"].as_str().unwrap().to_string();
    let mut ws = WsClient::connect(&server.stream_url(&run_id)).await;
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

    let payload = chat_request_payload_extended(
        &ws_path,
        ChatRequestOpts {
            temperature: Some(0.5),
            ..Default::default()
        },
    );
    let resp = post_runtime_request(&server, &payload).await;
    let body: Value = resp.json().await.unwrap();
    let run_id = body["run_id"].as_str().unwrap().to_string();
    let mut ws = WsClient::connect(&server.stream_url(&run_id)).await;
    let ready = ws.expect_session_ready().await;
    assert!(ready["session_id"].is_string());
}

#[tokio::test]
async fn ws_cfg_max_tokens() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("cfg-maxtok");
    std::fs::create_dir_all(&ws_path).unwrap();

    let payload = chat_request_payload_extended(
        &ws_path,
        ChatRequestOpts {
            max_tokens: Some(4096),
            ..Default::default()
        },
    );
    let resp = post_runtime_request(&server, &payload).await;
    let body: Value = resp.json().await.unwrap();
    let run_id = body["run_id"].as_str().unwrap().to_string();
    let mut ws = WsClient::connect(&server.stream_url(&run_id)).await;
    let ready = ws.expect_session_ready().await;
    assert!(ready["session_id"].is_string());
}

#[tokio::test]
async fn ws_cfg_project_id() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("cfg-projid");
    std::fs::create_dir_all(&ws_path).unwrap();

    let payload = chat_request_payload_extended(
        &ws_path,
        ChatRequestOpts {
            project_id: Some("proj-abc-123"),
            ..Default::default()
        },
    );
    let resp = post_runtime_request(&server, &payload).await;
    let body: Value = resp.json().await.unwrap();
    let run_id = body["run_id"].as_str().unwrap().to_string();
    let mut ws = WsClient::connect(&server.stream_url(&run_id)).await;
    let ready = ws.expect_session_ready().await;
    assert!(ready["session_id"].is_string());
}

#[tokio::test]
async fn ws_cfg_minimal_runtime_request() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("cfg-minimal");
    std::fs::create_dir_all(&ws_path).unwrap();

    let payload = chat_request_payload(&ws_path, None);
    let resp = post_runtime_request(&server, &payload).await;
    let body: Value = resp.json().await.unwrap();
    let run_id = body["run_id"].as_str().unwrap().to_string();
    let mut ws = WsClient::connect(&server.stream_url(&run_id)).await;
    let ready = ws.expect_session_ready().await;

    let tools = ready["tools"].as_array().unwrap();
    assert!(
        !tools.is_empty(),
        "should get default tools with minimal runtime request"
    );
}

#[tokio::test]
async fn ws_cfg_project_path_valid() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("cfg-projpath");
    std::fs::create_dir_all(&ws_path).unwrap();

    let real_dir = tempfile::tempdir().unwrap();

    let payload = chat_request_payload_extended(
        &ws_path,
        ChatRequestOpts {
            project_path: Some(real_dir.path().to_str().unwrap()),
            ..Default::default()
        },
    );
    let resp = post_runtime_request(&server, &payload).await;
    let body: Value = resp.json().await.unwrap();
    let run_id = body["run_id"].as_str().unwrap().to_string();
    let mut ws = WsClient::connect(&server.stream_url(&run_id)).await;
    let ready = ws.expect_session_ready().await;
    assert!(ready["session_id"].is_string());
}

#[tokio::test]
async fn ws_cfg_session_ready_includes_agent_and_core_tools() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("cfg-alltools");
    std::fs::create_dir_all(&ws_path).unwrap();

    let payload = chat_request_payload(&ws_path, None);
    let resp = post_runtime_request(&server, &payload).await;
    let body: Value = resp.json().await.unwrap();
    let run_id = body["run_id"].as_str().unwrap().to_string();
    let mut ws = WsClient::connect(&server.stream_url(&run_id)).await;
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
async fn ws_cfg_runtime_request_with_model_override() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("cfg-model");
    std::fs::create_dir_all(&ws_path).unwrap();

    let payload = chat_request_payload_extended(
        &ws_path,
        ChatRequestOpts {
            model: Some("claude-sonnet-4-20250514"),
            ..Default::default()
        },
    );
    let resp = post_runtime_request(&server, &payload).await;
    let body: Value = resp.json().await.unwrap();
    let run_id = body["run_id"].as_str().unwrap().to_string();
    let mut ws = WsClient::connect(&server.stream_url(&run_id)).await;
    let ready = ws.expect_session_ready().await;
    assert!(ready["session_id"].is_string());
}

#[tokio::test]
async fn ws_cfg_system_prompt_override() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("cfg-sysprompt");
    std::fs::create_dir_all(&ws_path).unwrap();

    let payload = chat_request_payload_extended(
        &ws_path,
        ChatRequestOpts {
            system_prompt: Some("You are a helpful pirate assistant."),
            ..Default::default()
        },
    );
    let resp = post_runtime_request(&server, &payload).await;
    let body: Value = resp.json().await.unwrap();
    let run_id = body["run_id"].as_str().unwrap().to_string();
    let mut ws = WsClient::connect(&server.stream_url(&run_id)).await;
    let ready = ws.expect_session_ready().await;
    assert!(ready["session_id"].is_string());
}

#[tokio::test]
async fn ws_cfg_multiple_installed_tools() {
    let server = start_mock_server().await;
    let ws_path = server.workspaces_path().join("cfg-multi-tools");
    std::fs::create_dir_all(&ws_path).unwrap();

    let payload = chat_request_payload_extended(
        &ws_path,
        ChatRequestOpts {
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
    );
    let resp = post_runtime_request(&server, &payload).await;
    let body: Value = resp.json().await.unwrap();
    let run_id = body["run_id"].as_str().unwrap().to_string();
    let mut ws = WsClient::connect(&server.stream_url(&run_id)).await;
    let ready = ws.expect_session_ready().await;

    let tools = ready["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"tool_alpha"), "missing tool_alpha");
    assert!(names.contains(&"tool_beta"), "missing tool_beta");
}
