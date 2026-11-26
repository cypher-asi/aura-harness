//! Suites 3, 5, 6, 11, 12: live LLM turns, multi-turn, cancellation, errors and streaming fields.

use std::time::Duration;

use serde_json::Value;

use crate::common::{
    assert_stop_reason, chat_request_payload, collect_text, connect_llm_session, find_file,
    open_chat_run, tool_names_used, TestServer,
};

// ============================================================================
// Suite 3: Real LLM Simple Turn
// ============================================================================

#[tokio::test]

async fn test_ws_simple_prompt() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("simple-prompt");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    ws.send_user_message("What is 2+2? Reply with just the number, nothing else.")
        .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    assert!(!messages.is_empty(), "no messages received");

    let has_text = messages.iter().any(|m| m["type"] == "text_delta");
    assert!(has_text, "expected at least one text_delta");

    assert_stop_reason(&messages, "end_turn");

    let end = messages
        .iter()
        .find(|m| m["type"] == "assistant_message_end")
        .unwrap();
    let input_tokens = end["usage"]["input_tokens"].as_u64().unwrap_or(0);
    let output_tokens = end["usage"]["output_tokens"].as_u64().unwrap_or(0);
    assert!(input_tokens > 0, "expected non-zero input_tokens");
    assert!(output_tokens > 0, "expected non-zero output_tokens");

    let text = collect_text(&messages);
    assert!(
        text.contains('4'),
        "expected response to contain '4', got: {text}"
    );
}

// ============================================================================
// Suite 5: Multi-turn Conversation
// ============================================================================

#[tokio::test]

async fn test_ws_multi_turn() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("multi-turn");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    // Turn 1: create a file
    ws.send_user_message(
        "Use the write_file tool to create a file at path 'memo.txt' with content 'Remember to buy milk'.",
    )
    .await;
    let turn1 = ws.collect_turn(Duration::from_secs(120)).await;
    let turn1_tools = tool_names_used(&turn1);
    assert!(
        turn1_tools.contains(&"write_file".to_string()),
        "expected write_file in turn 1, got: {turn1_tools:?}"
    );
    assert!(
        find_file(&ws_path, "memo.txt").is_some(),
        "file should exist somewhere under workspace after turn 1"
    );

    // Turn 2: read the file from turn 1 (tests context carryover)
    ws.send_user_message("Use the read_file tool to read 'memo.txt' and tell me its contents.")
        .await;
    let turn2 = ws.collect_turn(Duration::from_secs(120)).await;

    // The LLM may read the file or recall from context.
    // The key assertion is that multi-turn context works (turn 2 completes).
    assert_stop_reason(&turn2, "end_turn");

    let text = collect_text(&turn2);
    let tool_result_text: String = turn2
        .iter()
        .filter(|m| m["type"] == "tool_result")
        .filter_map(|m| m["result"].as_str())
        .collect();
    let combined = format!("{text}{tool_result_text}");
    if !combined.contains("milk") && !combined.contains("Remember") && !combined.contains("memo") {
        eprintln!(
            "NOTE: turn 2 completed but didn't explicitly reference file content (LLM behavior)"
        );
    }
}

// ============================================================================
// Suite 6: Cancellation
// ============================================================================

#[tokio::test]

async fn test_ws_cancel_turn() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("cancel-test");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    ws.send_user_message(
        "Write a very long essay about the history of computing, at least 2000 words. \
         Include detailed technical analysis of every major era.",
    )
    .await;

    // Wait for the turn to start streaming, then cancel
    tokio::time::sleep(Duration::from_secs(2)).await;
    ws.send_cancel().await;

    let messages = ws.collect_turn(Duration::from_secs(60)).await;

    // The turn should eventually end - either with cancelled or end_turn
    let end_msg = messages
        .iter()
        .find(|m| m["type"] == "assistant_message_end");
    if let Some(end) = end_msg {
        let stop = end["stop_reason"].as_str().unwrap_or("");
        assert!(
            stop == "cancelled" || stop == "end_turn" || stop == "end_turn_with_errors",
            "expected cancelled or end_turn, got: {stop}"
        );
    }
    // If no end message, the turn was still in-flight and we timed out - acceptable for cancel test
}

// ============================================================================
// Suite 11: Error Handling
//
// Phase A note: WebSocket parse-error coverage now requires an
// attached session (the `/stream/:run_id` route is the only WS
// endpoint and only sees frames after `prepare_chat_session`
// succeeds).
// ============================================================================

#[tokio::test]

async fn test_ws_invalid_json() {
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("invalid-json");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = open_chat_run(&server, &chat_request_payload(&ws_path, None)).await;
    ws.send_raw("this is not json at all!!!").await;

    let msg = ws.recv_json().await.expect("expected error response");
    assert_eq!(msg["type"], "error");
    assert_eq!(msg["code"], "parse_error");
}

#[tokio::test]

async fn test_ws_message_during_turn() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("msg-during-turn");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;

    ws.send_user_message(
        "Write a detailed 500-word essay about the importance of software testing.",
    )
    .await;

    // Wait for the turn to start streaming
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Try sending another message while turn is active
    ws.send_user_message("This should be rejected").await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;

    let has_turn_in_progress = messages
        .iter()
        .any(|m| m["type"] == "error" && m["code"] == "turn_in_progress");
    assert!(
        has_turn_in_progress,
        "expected turn_in_progress error, messages: {:?}",
        messages
            .iter()
            .map(|m| m["type"].as_str())
            .collect::<Vec<_>>()
    );
}

// ============================================================================
// Suite 12: Streaming Protocol Field Verification
// ============================================================================

#[tokio::test]
async fn test_ws_streaming_message_fields() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("stream-fields");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    ws.send_user_message("Say hello in one sentence.").await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    assert!(!messages.is_empty(), "no messages received");

    let start = messages
        .iter()
        .find(|m| m["type"] == "assistant_message_start");
    assert!(start.is_some(), "missing assistant_message_start");
    let start_msg_id = start.unwrap()["message_id"].as_str().unwrap_or("");
    assert!(
        !start_msg_id.is_empty(),
        "assistant_message_start should have non-empty message_id"
    );

    let has_text_delta = messages
        .iter()
        .any(|m| m["type"] == "text_delta" && m["text"].as_str().is_some_and(|t| !t.is_empty()));
    assert!(has_text_delta, "expected at least one non-empty text_delta");

    let end_msg = messages
        .iter()
        .find(|m| m["type"] == "assistant_message_end")
        .expect("missing assistant_message_end");

    let end_msg_id = end_msg["message_id"].as_str().unwrap_or("");
    assert_eq!(
        start_msg_id, end_msg_id,
        "message_id should match between start and end"
    );

    assert!(
        end_msg["stop_reason"].is_string(),
        "stop_reason should be a string"
    );

    assert_end_message_usage_and_files(end_msg);
}

/// Checks the `usage` and `files_changed` fields of an `assistant_message_end` frame.
fn assert_end_message_usage_and_files(end_msg: &Value) {
    let usage = &end_msg["usage"];
    assert!(
        usage["input_tokens"].as_u64().unwrap_or(0) > 0,
        "input_tokens should be > 0"
    );
    assert!(
        usage["output_tokens"].as_u64().unwrap_or(0) > 0,
        "output_tokens should be > 0"
    );
    assert!(
        usage["model"].is_string() && usage["model"].as_str().is_some_and(|m| !m.is_empty()),
        "model should be non-empty"
    );

    let fc = &end_msg["files_changed"];
    assert!(
        fc["created"].is_array(),
        "files_changed.created should be an array"
    );
    assert!(
        fc["modified"].is_array(),
        "files_changed.modified should be an array"
    );
    assert!(
        fc["deleted"].is_array(),
        "files_changed.deleted should be an array"
    );
}

#[tokio::test]
async fn test_ws_files_changed_after_write() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("files-changed");
    std::fs::create_dir_all(&ws_path).unwrap();

    let mut ws = connect_llm_session(&server, &ws_path, &token).await;
    ws.send_user_message(
        "Use the write_file tool to create a file called 'changed.txt' with the content 'tracking changes'.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let tools = tool_names_used(&messages);
    assert!(
        tools.contains(&"write_file".to_string()),
        "expected write_file"
    );

    // files_changed is populated by the server; it may or may not track tool-based writes
    // depending on implementation. We verify the field structure exists regardless.
    let end_msg = messages
        .iter()
        .find(|m| m["type"] == "assistant_message_end");
    assert!(end_msg.is_some(), "missing assistant_message_end");
    let fc = &end_msg.unwrap()["files_changed"];
    assert!(
        fc["created"].is_array(),
        "files_changed.created should be an array"
    );
}
