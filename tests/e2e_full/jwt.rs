//! Suite 5: ZOS login and JWT authentication.

use crate::require_zos;
use crate::support::*;

// ============================================================================
// Suite 5: ZOS Login + JWT Authentication
//
// Phase A note: the `jwt_via_session_init_token` test was deleted —
// the `SessionInit.token` field no longer exists; the model JWT now
// rides under `RuntimeRequest.auth_jwt` and is forwarded by the
// chat-session bootstrap. The `jwt_via_bearer_header` variant
// continues to exercise the alternative path where the bearer is
// supplied on the transport instead of in the request body.
// ============================================================================

#[tokio::test]
async fn zos_login_obtains_jwt() {
    let _ = aura_config::env_file::load();
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
    let _ = aura_config::env_file::load();
    // Only run if we have valid creds (so we know the endpoint is reachable)
    let _valid_creds = require_zos!();

    let result = common::zos_login("bad-email@nonexistent.test", "wrong-password").await;
    assert!(result.is_err(), "bad credentials should fail");
}

#[tokio::test]
async fn zos_whoami_after_login() {
    let _ = aura_config::env_file::load();
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
    let _ = aura_config::env_file::load();
    let (email, password) = require_zos!();
    let token = common::zos_login(&email, &password).await.unwrap();

    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("jwt-bearer");
    std::fs::create_dir_all(&ws_path).unwrap();

    // Phase A: drive the bootstrap with the JWT supplied on the HTTP
    // bearer (both the POST and the WS upgrade) instead of in the
    // request body — exercises the path where the transport carries
    // the credential and the gateway forwards it to the run.
    let payload = chat_request_payload(&ws_path, None);
    let mut ws = common::open_chat_run_with_auth(&server, &payload, &token).await;

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
async fn jwt_llm_turn() {
    let _ = aura_config::env_file::load();
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
async fn jwt_tool_turn() {
    let _ = aura_config::env_file::load();
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
async fn jwt_missing_errors() {
    let _ = aura_config::env_file::load();
    let _creds = require_zos!();

    let server = TestServer::start().await;
    let ws_path = server.workspaces_path().join("jwt-missing");
    std::fs::create_dir_all(&ws_path).unwrap();

    // Open the run with no JWT supplied (neither in `auth_jwt` nor on
    // the HTTP bearer — the bearer header on the POST goes to the
    // node auth gate, not the model router).
    let payload = chat_request_payload(&ws_path, None);
    let mut ws = open_chat_run(&server, &payload).await;

    ws.send_user_message("hello").await;

    let messages = ws.collect_turn(Duration::from_secs(60)).await;
    // Should get an error about missing JWT (the only LLM path is the
    // router proxy, which requires a Bearer token).
    let has_error = messages
        .iter()
        .any(|m| m["type"] == "error" || m["stop_reason"] == "end_turn_with_errors");
    assert!(
        has_error,
        "missing JWT should produce an error or error stop_reason"
    );
}
