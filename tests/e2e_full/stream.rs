//! Suite 7: streaming protocol fidelity.

use crate::support::*;

// ============================================================================
// Suite 7: Streaming Protocol Fidelity
// ============================================================================

#[tokio::test]
async fn stream_message_sequence_order() {
    let _ = aura_config::env_file::load();
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
    let _ = aura_config::env_file::load();
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
    let _ = aura_config::env_file::load();
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
    let _ = aura_config::env_file::load();
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
    let _ = aura_config::env_file::load();
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
    let _ = aura_config::env_file::load();
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
    let _ = aura_config::env_file::load();
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
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("stream-stop");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    ws.send_user_message("What is 1+1?").await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    assert_stop_reason(&messages, "end_turn");
}
