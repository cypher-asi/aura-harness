use super::*;
use axum::extract::Query;
use axum::routing::get;
use axum::Json;
use reqwest::header::{HeaderMap, HeaderValue};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

#[test]
fn persisted_header_accepts_true() {
    let mut headers = HeaderMap::new();
    headers.insert(CHAT_PERSISTED_HEADER, HeaderValue::from_static("true"));
    assert!(require_persisted_header(&headers).is_ok());
}

#[test]
fn persisted_header_rejects_false() {
    let mut headers = HeaderMap::new();
    headers.insert(CHAT_PERSISTED_HEADER, HeaderValue::from_static("false"));
    let err = require_persisted_header(&headers).unwrap_err();
    assert!(err.contains("reported"), "got: {err}");
}

#[test]
fn persisted_header_rejects_missing() {
    let err = require_persisted_header(&HeaderMap::new()).unwrap_err();
    assert!(err.contains("missing"), "got: {err}");
}

/// Cross-repo regression: pin that `list_agents` always asks
/// aura-os-server for the slim shape via `?view=slim`. Without
/// this, the full `Vec<Agent>` payload (multi-KB icons +
/// system_prompt per row) overflows the runner's per-tool-result
/// cap and truncates the JSON before agent names are reachable —
/// which manifested as "every list_agents call gets truncated
/// before I reach the name fields" on the LLM side. The matching
/// server endpoint is documented at
/// `apps/aura-os-server/src/handlers/agents/crud/list.rs`
/// (`AgentListView`); if either side drifts, this test fails first.
async fn spawn_capturing_mock(captured: Arc<Mutex<Option<HashMap<String, String>>>>) -> String {
    let captured_for_route = captured.clone();
    let app = axum::Router::new().route(
        "/api/agents",
        get(move |Query(params): Query<HashMap<String, String>>| {
            let captured = captured_for_route.clone();
            async move {
                *captured.lock().await = Some(params);
                Json(serde_json::json!([]))
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn list_agents_callback_always_requests_view_slim() {
    let captured = Arc::new(Mutex::new(None::<HashMap<String, String>>));
    let base_url = spawn_capturing_mock(captured.clone()).await;

    let hook = AuraServerAgentHook::new(base_url, Some("test-jwt".into()));
    hook.list_agents(None).await.expect("list_agents call");

    let params = captured.lock().await.clone().expect("captured params");
    assert_eq!(
        params.get("view").map(String::as_str),
        Some("slim"),
        "list_agents must request view=slim from aura-os-server; got {params:?}"
    );
}

#[tokio::test]
async fn list_agents_callback_combines_view_slim_with_org_id() {
    let captured = Arc::new(Mutex::new(None::<HashMap<String, String>>));
    let base_url = spawn_capturing_mock(captured.clone()).await;

    let hook = AuraServerAgentHook::new(base_url, Some("test-jwt".into()));
    hook.list_agents(Some("org-1"))
        .await
        .expect("list_agents call");

    let params = captured.lock().await.clone().expect("captured params");
    assert_eq!(
        params.get("view").map(String::as_str),
        Some("slim"),
        "view=slim must accompany org_id; got {params:?}"
    );
    assert_eq!(
        params.get("org_id").map(String::as_str),
        Some("org-1"),
        "org_id must be forwarded alongside view=slim; got {params:?}"
    );
}

/// Spin up a mock `POST /api/agents/:agent_id/events/stream` endpoint
/// that captures the inbound JSON body, returns a `200` with the
/// `x-aura-chat-persisted: true` header so `deliver_message` returns
/// `Ok(())`, and reports back the body through the shared `Mutex`.
///
/// Header construction uses `axum::http` types directly (rather than
/// the `reqwest::header` aliases imported at the top of the module
/// for header inspection in `require_persisted_header` tests) because
/// axum 0.7 builds on `http` 1.x and rejects the older
/// `reqwest::header::HeaderValue` types at the response builder.
async fn spawn_capturing_chat_stream_mock(captured: Arc<Mutex<Option<Value>>>) -> String {
    use axum::extract::Path;
    use axum::http::header::HeaderName as AxumHeaderName;
    use axum::http::HeaderMap as AxumHeaderMap;
    use axum::http::HeaderValue as AxumHeaderValue;
    use axum::http::StatusCode;
    use axum::routing::post;

    let captured_for_route = captured.clone();
    let app = axum::Router::new().route(
        "/api/agents/:agent_id/events/stream",
        post(
            move |Path(_agent_id): Path<String>, axum::Json(body): axum::Json<Value>| {
                let captured = captured_for_route.clone();
                async move {
                    *captured.lock().await = Some(body);
                    let mut headers = AxumHeaderMap::new();
                    headers.insert(
                        AxumHeaderName::from_static(CHAT_PERSISTED_HEADER),
                        AxumHeaderValue::from_static("true"),
                    );
                    (StatusCode::OK, headers, "")
                }
            },
        ),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// Cross-repo regression: pin that the cross-agent `deliver_message`
/// flows the caller's agent id into the outbound POST as
/// `originating_agent_id`. The server-side async reply wiring
/// (`spawn_cross_agent_reply_callback` in
/// `apps/aura-os-server/src/handlers/agents/chat/persist_task.rs`)
/// keys on this field to post a follow-up `user_message` back into
/// the originating agent's session when the target's turn finishes,
/// so if either side drops the field the async reply chain breaks
/// silently.
#[tokio::test]
async fn deliver_message_forwards_parent_agent_id_as_originating_agent_id() {
    let captured = Arc::new(Mutex::new(None::<Value>));
    let base_url = spawn_capturing_chat_stream_mock(captured.clone()).await;

    let hook = AuraServerAgentHook::new(base_url, Some("test-jwt".into()));
    hook.deliver_message(AgentMessage {
        parent_agent_id: Some("caller-agent"),
        originating_user_id: Some("user-root"),
        project_id: Some("project-123"),
        model: Some("aura-claude-sonnet-4-6"),
        ..AgentMessage::new("target-agent", "hello")
    })
    .await
    .expect("deliver_message call");

    let body = captured.lock().await.clone().expect("captured body");
    assert_eq!(
        body.get("originating_agent_id").and_then(Value::as_str),
        Some("caller-agent"),
        "deliver_message must forward parent_agent_id as `originating_agent_id` so \
         aura-os-server can post the target's reply back into the caller's session; \
         got: {body}"
    );
    assert_eq!(
        body.get("project_id").and_then(Value::as_str),
        Some("project-123"),
        "deliver_message must preserve the originating project; got: {body}"
    );
    assert_eq!(
        body.get("content").and_then(Value::as_str),
        Some("hello"),
        "content must be threaded through verbatim; got: {body}"
    );
    assert_eq!(
        body.get("new_session").and_then(Value::as_bool),
        Some(false),
        "deliver_message must always join the target's existing chat session; got: {body}"
    );
    assert_eq!(
        body.get("model").and_then(Value::as_str),
        Some("aura-claude-sonnet-4-6"),
        "deliver_message must forward the caller's model so the target turn runs on a \
         real model instead of failing with an empty model; got: {body}"
    );
}

/// Companion to the above: when the caller agent id is unknown
/// (e.g. the tool was invoked from a context without
/// `caller_agent_id`), the POST body still includes
/// `originating_agent_id`, but as a JSON `null`. This pins the
/// nullable shape so the server's serde deserializer
/// (`Option<String>` with `#[serde(default)]`) keeps accepting it.
#[tokio::test]
async fn deliver_message_serializes_missing_parent_as_null() {
    let captured = Arc::new(Mutex::new(None::<Value>));
    let base_url = spawn_capturing_chat_stream_mock(captured.clone()).await;

    let hook = AuraServerAgentHook::new(base_url, Some("test-jwt".into()));
    hook.deliver_message(AgentMessage::new("target-agent", "hello"))
        .await
        .expect("deliver_message call");

    let body = captured.lock().await.clone().expect("captured body");
    assert!(
        body.get("originating_agent_id").is_some_and(Value::is_null),
        "missing parent_agent_id must serialize as a JSON null; got: {body}"
    );
    assert!(
        body.get("from_agent_id").is_some_and(Value::is_null),
        "missing parent_agent_id must also null out from_agent_id so the \
         recipient's chat panel does not render a meaningless badge; got: {body}"
    );
    assert!(
        body.get("model").is_some_and(Value::is_null),
        "a missing caller model must serialize as a JSON null so the server \
         falls back to the target agent's configured model; got: {body}"
    );
}

/// Cross-repo regression: when `parent_agent_id` is set, the
/// outbound POST must carry it on BOTH `originating_agent_id`
/// (server-side async-reply routing) AND `from_agent_id`
/// (display-side cross-agent provenance for the recipient's
/// chat panel). The two wire fields are kept distinct on
/// purpose — the server's B→A reply leg nulls out
/// `originating_agent_id` for the single-hop fall-off but still
/// sets `from_agent_id: <B>` so the badge UI works on both
/// directions of the round trip. Pairs with the server-side
/// pin in
/// `apps/aura-os-server/tests/cross_agent_reply_callback_test.rs`
/// (`from_agent_id` assertion in
/// `cross_agent_callback_posts_reply_*`).
#[tokio::test]
async fn deliver_message_forwards_parent_agent_id_as_from_agent_id() {
    let captured = Arc::new(Mutex::new(None::<Value>));
    let base_url = spawn_capturing_chat_stream_mock(captured.clone()).await;

    let hook = AuraServerAgentHook::new(base_url, Some("test-jwt".into()));
    hook.deliver_message(AgentMessage {
        parent_agent_id: Some("caller-agent"),
        originating_user_id: Some("user-root"),
        ..AgentMessage::new("target-agent", "hello")
    })
    .await
    .expect("deliver_message call");

    let body = captured.lock().await.clone().expect("captured body");
    assert_eq!(
        body.get("from_agent_id").and_then(Value::as_str),
        Some("caller-agent"),
        "deliver_message must forward parent_agent_id on `from_agent_id` so \
         the recipient's chat panel can render a `↩ from <agent>` badge \
         on the inbound row instead of letting the message look like a \
         real user prompt; got: {body}"
    );
    // Sanity: the legacy routing field still carries the same id.
    // The server uses this to know where to POST the async reply
    // back to; if either field drops while the other stays, the
    // round-trip-display contract breaks asymmetrically.
    assert_eq!(
        body.get("originating_agent_id").and_then(Value::as_str),
        Some("caller-agent"),
        "originating_agent_id must continue to carry the same id for \
         server-side async-reply routing; got: {body}"
    );
}
