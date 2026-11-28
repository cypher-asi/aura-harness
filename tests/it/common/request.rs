//! `RuntimeRequest` payload builders and `POST /v1/run` + WS attach helpers.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};

use super::auth::{default_agent_permissions_payload, http_client, E2E_TEST_BEARER};
use super::server::TestServer;
use super::ws::WsClient;

// ============================================================================
// RuntimeRequest builders (Phase A wire shape)
// ============================================================================

/// Build a minimal chat-kind [`aura_core_protocol::RuntimeRequest`] payload.
///
/// Fields the harness validates:
///
/// - `workspace.workspace` must live under the node's workspaces base
///   (the chat-session bootstrap rejects anything else with the
///   `invalid_workspace` error code, surfaced as HTTP 400).
/// - `user_id` must be non-empty — the new wire shape requires the
///   originating user id up-front rather than minting one on the
///   server side.
pub fn chat_request_payload(workspace: &Path, token: Option<&str>) -> Value {
    chat_request_payload_full(workspace, token, None, None, None, None)
}

pub fn chat_request_payload_full(
    workspace: &Path,
    token: Option<&str>,
    system_prompt: Option<&str>,
    model: Option<&str>,
    max_tokens: Option<u32>,
    max_turns: Option<u32>,
) -> Value {
    chat_request_payload_extended(
        workspace,
        ChatRequestOpts {
            token,
            system_prompt,
            model,
            max_tokens,
            max_turns,
            ..Default::default()
        },
    )
}

/// Build a chat [`aura_core_protocol::RuntimeRequest`] payload from a
/// full options bag. Mirrors what the wire layer accepts; every
/// `None` field is omitted so the harness's serde defaults apply.
pub fn chat_request_payload_extended(workspace: &Path, mut opts: ChatRequestOpts<'_>) -> Value {
    let mut request = json!({
        "type": {
            "kind": "chat",
            "params": {
                "conversation_messages": opts.conversation_messages.take().unwrap_or_default(),
            }
        },
        "agent_identity": {},
        "model": {},
        "workspace": {
            "workspace": workspace.to_string_lossy(),
        },
        "agent_permissions": opts
            .agent_permissions
            .take()
            .unwrap_or_else(default_agent_permissions_payload),
        "agent_capabilities": {},
        "user_id": opts.user_id.unwrap_or("e2e-user").to_string(),
    });
    apply_identity_and_model_opts(&mut request, &opts);
    apply_project_and_capability_opts(&mut request, opts);
    request
}

fn apply_identity_and_model_opts(request: &mut Value, opts: &ChatRequestOpts<'_>) {
    if let Some(t) = opts.token {
        request["auth_jwt"] = json!(t);
    }
    if let Some(sp) = opts.system_prompt {
        request["agent_identity"]["system_prompt"] = json!(sp);
    }
    if let Some(pid) = opts.partition_id {
        request["agent_identity"]["partition_id"] = json!(pid);
    }
    if let Some(m) = opts.model {
        request["model"]["id"] = json!(m);
    }
    if let Some(mt) = opts.max_tokens {
        request["model"]["max_tokens"] = json!(mt);
    }
    let max_turns = opts.max_turns.unwrap_or(aura_core_types::MAX_TURNS);
    request["model"]["max_turns"] = json!(max_turns);
    if let Some(temp) = opts.temperature {
        request["model"]["temperature"] = json!(temp);
    }
}

fn apply_project_and_capability_opts(request: &mut Value, opts: ChatRequestOpts<'_>) {
    if let Some(pid) = opts.project_id {
        request["project"] = json!({ "project_id": pid });
    }
    if let Some(pp) = opts.project_path {
        request["workspace"]["project_path"] = json!(pp);
    }
    if let Some(tools) = opts.installed_tools {
        request["agent_capabilities"]["installed_tools"] = json!(tools);
    }
    if let Some(integrations) = opts.installed_integrations {
        request["agent_capabilities"]["installed_integrations"] = json!(integrations);
    }
}

/// Options bag for [`chat_request_payload_extended`].
#[derive(Default)]
pub struct ChatRequestOpts<'a> {
    pub token: Option<&'a str>,
    pub system_prompt: Option<&'a str>,
    pub model: Option<&'a str>,
    pub max_tokens: Option<u32>,
    pub max_turns: Option<u32>,
    pub temperature: Option<f32>,
    pub project_id: Option<&'a str>,
    pub project_path: Option<&'a str>,
    pub installed_tools: Option<Vec<Value>>,
    pub installed_integrations: Option<Vec<Value>>,
    pub conversation_messages: Option<Vec<Value>>,
    pub user_id: Option<&'a str>,
    /// Override the default full-access `agent_permissions` payload.
    pub agent_permissions: Option<Value>,
    /// Pin the session's `agent_id` via `agent_identity.partition_id`.
    /// The runtime parses this as a hex `AgentId` (falling back to a
    /// blake3 hash of the string), so a test can both register an
    /// identity for, and later address `/tx` / `/record` / `/head` to,
    /// the same agent.
    pub partition_id: Option<&'a str>,
}

/// `POST /v1/run` with the given body and return the parsed
/// `RuntimeRunResponse` JSON `{run_id, event_stream_url}`. Bearer
/// auth is supplied via [`http_client`] (constant-time test token).
pub async fn post_runtime_request(server: &TestServer, payload: &Value) -> reqwest::Response {
    http_client()
        .post(format!("{}/v1/run", server.base_url()))
        .json(payload)
        .send()
        .await
        .expect("POST /v1/run")
}

/// Same as [`post_runtime_request`] but using a no-auth client.
/// Used by the few tests that exercise the unauthenticated path.
pub async fn post_runtime_request_anonymous(
    server: &TestServer,
    payload: &Value,
) -> reqwest::Response {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap()
        .post(format!("{}/v1/run", server.base_url()))
        .json(payload)
        .send()
        .await
        .expect("POST /v1/run (anonymous)")
}

/// Assert the `POST /v1/run` succeeded and return the minted `run_id`.
async fn expect_run_id(resp: reqwest::Response) -> String {
    let status = resp.status();
    let body: Value = resp.json().await.expect("POST /v1/run json body");
    assert!(
        status.is_success(),
        "POST /v1/run failed with status {status}: {body}"
    );
    body["run_id"]
        .as_str()
        .expect("RuntimeRunResponse missing run_id")
        .to_string()
}

/// Drive the two-step chat-session bootstrap:
/// `POST /v1/run` (default test bearer) → `WS /stream/:run_id` →
/// expect `SessionReady` as the first server frame.
///
/// Returns a ready-to-drive [`WsClient`] already past the
/// `SessionReady` handshake. Panics with the HTTP body if the POST
/// rejects the request.
pub async fn open_chat_run(server: &TestServer, payload: &Value) -> WsClient {
    let resp = post_runtime_request(server, payload).await;
    let run_id = expect_run_id(resp).await;
    let stream_url = server.stream_url(&run_id);
    let mut ws = WsClient::connect_with_auth(&stream_url, E2E_TEST_BEARER).await;
    ws.expect_session_ready().await;
    ws
}

/// Same as [`open_chat_run`] but the WS is opened without a Bearer
/// header. The `POST /v1/run` step still uses the default test
/// bearer because the gateway's `require_bearer_mw` middleware
/// rejects unauthenticated runs of either kind today.
pub async fn open_chat_run_anonymous_ws(server: &TestServer, payload: &Value) -> WsClient {
    let resp = post_runtime_request(server, payload).await;
    let run_id = expect_run_id(resp).await;
    let stream_url = server.stream_url(&run_id);
    let mut ws = WsClient::connect_anonymous(&stream_url).await;
    ws.expect_session_ready().await;
    ws
}

/// Same as [`open_chat_run`] but uses an explicit bearer token for
/// both the POST and the WS upgrade. The wire shape carries the
/// model proxy / domain JWT separately under `auth_jwt`; this
/// helper only sets the *transport* bearer, mirroring how a real
/// frontend uses one credential for the HTTP gate and threads the
/// upstream JWT through the request body.
pub async fn open_chat_run_with_auth(
    server: &TestServer,
    payload: &Value,
    bearer: &str,
) -> WsClient {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .default_headers({
            let mut h = reqwest::header::HeaderMap::new();
            h.insert(
                reqwest::header::AUTHORIZATION,
                reqwest::header::HeaderValue::from_str(&format!("Bearer {bearer}")).unwrap(),
            );
            h
        })
        .build()
        .unwrap();
    let resp = client
        .post(format!("{}/v1/run", server.base_url()))
        .json(payload)
        .send()
        .await
        .expect("POST /v1/run");
    let run_id = expect_run_id(resp).await;
    let stream_url = server.stream_url(&run_id);
    let mut ws = WsClient::connect_with_auth(&stream_url, bearer).await;
    ws.expect_session_ready().await;
    ws
}

/// Convenience: drive the chat-session bootstrap end-to-end against
/// the live LLM router.
///
/// Phase A flow: build a [`chat_request_payload`] with the supplied
/// JWT in `auth_jwt`, POST to `/v1/run`, attach `/stream/:run_id`,
/// and wait for `SessionReady`. Returns the attached [`WsClient`].
pub async fn connect_llm_session(server: &TestServer, ws_path: &Path, token: &str) -> WsClient {
    let tok = if token.is_empty() { None } else { Some(token) };
    let payload = chat_request_payload(ws_path, tok);
    open_chat_run(server, &payload).await
}
