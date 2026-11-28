//! Suite 13: session config overrides and bearer auth.

use std::time::Duration;

use crate::common::{
    self, assert_stop_reason, chat_request_payload, chat_request_payload_full, collect_text,
    open_chat_run, place_file_in_agent_dir, TestServer,
};

// ============================================================================
// Suite 13: Session Config Overrides
// ============================================================================

#[tokio::test]
async fn test_ws_runtime_request_with_system_prompt() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("sys-prompt");
    std::fs::create_dir_all(&ws_path).unwrap();

    let tok = if token.is_empty() {
        None
    } else {
        Some(token.as_str())
    };
    let payload = chat_request_payload_full(
        &ws_path,
        tok,
        Some("You are a pirate. Every response must contain the word 'arrr'. This is mandatory."),
        None,
        None,
        None,
    );
    let mut ws = open_chat_run(&server, &payload).await;

    ws.send_user_message("Say hello.").await;
    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    assert_stop_reason(&messages, "end_turn");

    let text = collect_text(&messages).to_lowercase();
    assert!(
        text.contains("arrr")
            || text.contains("ahoy")
            || text.contains("matey")
            || text.contains("pirate"),
        "system_prompt override should influence response, got: {text}"
    );
}

#[tokio::test]
async fn test_ws_runtime_request_with_model() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("model-override");
    std::fs::create_dir_all(&ws_path).unwrap();

    // Pin to the env-fallback model identifier — this end-to-end test
    // exercises the wire-protocol round trip, not model selection logic.
    let model = aura_model::ENV_FALLBACK_MODEL;
    let tok = if token.is_empty() {
        None
    } else {
        Some(token.as_str())
    };
    let payload = chat_request_payload_full(&ws_path, tok, None, Some(model), None, None);
    let mut ws = open_chat_run(&server, &payload).await;

    ws.send_user_message("Say hi.").await;
    let messages = ws.collect_turn(Duration::from_secs(120)).await;

    let end_msg = messages
        .iter()
        .find(|m| m["type"] == "assistant_message_end")
        .expect("missing assistant_message_end");
    let used_model = end_msg["usage"]["model"].as_str().unwrap_or("");
    assert!(!used_model.is_empty(), "usage.model should be non-empty");
}

#[tokio::test]
async fn test_ws_runtime_request_with_max_turns() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("max-turns");
    std::fs::create_dir_all(&ws_path).unwrap();

    let tok = if token.is_empty() {
        None
    } else {
        Some(token.as_str())
    };
    let payload = chat_request_payload_full(&ws_path, tok, None, None, None, Some(1));
    let mut ws = open_chat_run(&server, &payload).await;

    place_file_in_agent_dir(&ws_path, "a.txt", "aaa");
    place_file_in_agent_dir(&ws_path, "b.txt", "bbb");

    ws.send_user_message(
        "Read the files a.txt, b.txt, then list all files, then create a summary file. \
         Use the tools for each step.",
    )
    .await;

    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    let end_msg = messages
        .iter()
        .find(|m| m["type"] == "assistant_message_end");
    assert!(
        end_msg.is_some(),
        "should get assistant_message_end even with max_turns=1"
    );
}

#[tokio::test]
async fn test_ws_auth_bearer_header() {
    let _ = aura_config::env_file::load();
    let token = require_llm!();
    assert!(
        !token.is_empty(),
        "bearer header test requires a non-empty token"
    );
    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("bearer-header");
    std::fs::create_dir_all(&ws_path).unwrap();

    // Phase A: the model JWT can either ride in the `auth_jwt` body
    // field or as a transport bearer that the gateway threads through.
    // This test exercises the bearer-header path: the POST + WS both
    // use the JWT as their transport credential, and the body's
    // `auth_jwt` is left unset.
    let payload = chat_request_payload(&ws_path, None);
    let mut ws = common::open_chat_run_with_auth(&server, &payload, &token).await;

    ws.send_user_message("Say hello in one word.").await;
    let messages = ws.collect_turn(Duration::from_secs(120)).await;
    assert!(
        !messages.is_empty(),
        "should receive messages with bearer header auth"
    );

    let has_text = messages.iter().any(|m| m["type"] == "text_delta");
    assert!(has_text, "should get text_delta with bearer header auth");
}
