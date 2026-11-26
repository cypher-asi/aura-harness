
use super::*;

use std::net::SocketAddr;
use std::sync::Arc as StdArc;

use aura_core_types::{ActionId, AgentId};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::sandbox::Sandbox;
use crate::ToolConfig;

/// Captured HTTP request, populated by the mock server.
#[derive(Default, Debug, Clone)]
struct Captured {
    method: String,
    path_and_query: String,
    headers: Vec<(String, String)>,
    body: String,
}

/// A dead-simple single-connection HTTP/1.1 mock. Reads one
/// request, captures it, responds with the configured status/body.
async fn start_mock(
    status: u16,
    response_body: &'static str,
) -> (
    SocketAddr,
    StdArc<Mutex<Captured>>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let captured: StdArc<Mutex<Captured>> = StdArc::new(Mutex::new(Captured::default()));
    let captured_clone = captured.clone();

    let handle = tokio::spawn(async move {
        let Ok((mut sock, _)) = listener.accept().await else {
            return;
        };

        let mut buf = vec![0u8; 16 * 1024];
        let n = sock.read(&mut buf).await.unwrap_or(0);
        let raw = String::from_utf8_lossy(&buf[..n]).to_string();

        let mut cap = captured_clone.lock().await;
        if let Some(first_line_end) = raw.find("\r\n") {
            let start_line = &raw[..first_line_end];
            let mut parts = start_line.split_whitespace();
            if let (Some(m), Some(p)) = (parts.next(), parts.next()) {
                cap.method = m.to_string();
                cap.path_and_query = p.to_string();
            }
        }
        if let Some(header_end) = raw.find("\r\n\r\n") {
            let headers_block = &raw[raw.find("\r\n").unwrap() + 2..header_end];
            for line in headers_block.split("\r\n") {
                if let Some((n, v)) = line.split_once(':') {
                    cap.headers
                        .push((n.trim().to_ascii_lowercase(), v.trim().to_string()));
                }
            }
            cap.body = raw[header_end + 4..].to_string();
        }
        drop(cap);

        let reason = if status == 200 { "OK" } else { "Error" };
        let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
        let _ = sock.write_all(response.as_bytes()).await;
        let _ = sock.shutdown().await;
    });

    (addr, captured, handle)
}

fn ctx() -> ToolContext {
    let dir = std::env::temp_dir();
    ToolContext::new(Sandbox::new(&dir).unwrap(), ToolConfig::default())
}

fn dummy_agent() -> (AgentId, ActionId) {
    (AgentId::generate(), ActionId::generate())
}

#[tokio::test]
async fn post_body_includes_json_args_and_bearer_token() {
    let (addr, cap, _h) = start_mock(200, r#"{"ok":true}"#).await;
    let tool = HttpToolDefinition::builder(
        "create_project",
        "Create a project.",
        json!({"type": "object", "properties": {"name": {"type": "string"}}}),
        format!("http://{addr}"),
        "/api/projects",
    )
    .method(HttpMethod::Post)
    .auth(HttpAuthSource::StaticBearer("jwt-xyz".to_string()))
    .try_build()
    .expect("tool must build in tests");

    let _ = dummy_agent();
    let result = tool
        .execute(&ctx(), json!({"name": "hello"}))
        .await
        .unwrap();
    assert!(result.ok);
    assert_eq!(&result.stdout[..], br#"{"ok":true}"#);

    let captured = cap.lock().await;
    assert_eq!(captured.method, "POST");
    assert_eq!(captured.path_and_query, "/api/projects");

    let auth = captured
        .headers
        .iter()
        .find(|(n, _)| n == "authorization")
        .map(|(_, v)| v.clone())
        .expect("Authorization header sent");
    assert_eq!(auth, "Bearer jwt-xyz");

    let content_type = captured
        .headers
        .iter()
        .find(|(n, _)| n == "content-type")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    assert!(content_type.starts_with("application/json"));
    assert!(captured.body.contains("\"name\":\"hello\""));
}

#[tokio::test]
async fn url_placeholder_is_filled_and_removed_from_body() {
    let (addr, cap, _h) = start_mock(200, "{}").await;
    let tool = HttpToolDefinition::builder(
        "update_project",
        "Rename a project.",
        json!({"type": "object"}),
        format!("http://{addr}"),
        "/api/projects/{project_id}",
    )
    .method(HttpMethod::Put)
    .try_build()
    .expect("tool must build in tests");

    let _ = tool
        .execute(&ctx(), json!({"project_id": "abc-123", "name": "new"}))
        .await
        .unwrap();

    let captured = cap.lock().await;
    assert_eq!(captured.method, "PUT");
    assert_eq!(captured.path_and_query, "/api/projects/abc-123");
    assert!(captured.body.contains("\"name\":\"new\""));
    assert!(!captured.body.contains("project_id"));
}

#[tokio::test]
async fn get_promotes_remaining_args_to_query_params() {
    let (addr, cap, _h) = start_mock(200, "[]").await;
    let tool = HttpToolDefinition::builder(
        "list_projects",
        "List projects in the org.",
        json!({"type": "object"}),
        format!("http://{addr}"),
        "/api/orgs/{org_id}/projects",
    )
    .method(HttpMethod::Get)
    .try_build()
    .expect("tool must build in tests");

    let _ = tool
        .execute(
            &ctx(),
            json!({"org_id": "o-1", "archived": false, "limit": 10}),
        )
        .await
        .unwrap();

    let captured = cap.lock().await;
    assert_eq!(captured.method, "GET");
    assert!(captured
        .path_and_query
        .starts_with("/api/orgs/o-1/projects?"));
    assert!(captured.path_and_query.contains("archived=false"));
    assert!(captured.path_and_query.contains("limit=10"));
}

#[tokio::test]
async fn non_2xx_maps_to_tool_result_failure() {
    let (addr, _cap, _h) = start_mock(500, r#"{"error":"boom"}"#).await;
    let tool = HttpToolDefinition::builder(
        "archive_project",
        "Archive a project.",
        json!({"type": "object"}),
        format!("http://{addr}"),
        "/api/projects/{project_id}/archive",
    )
    .method(HttpMethod::Post)
    .try_build()
    .expect("tool must build in tests");

    let result = tool
        .execute(&ctx(), json!({"project_id": "p-1"}))
        .await
        .unwrap();
    assert!(!result.ok);
    assert_eq!(result.exit_code, Some(500));
    assert_eq!(&result.stderr[..], br#"{"error":"boom"}"#);
    assert_eq!(
        result.metadata.get("http_status").map(String::as_str),
        Some("500")
    );
}

#[tokio::test]
async fn dynamic_auth_source_is_consulted_per_call() {
    let (addr, cap, _h) = start_mock(200, "{}").await;
    let token_slot: StdArc<std::sync::Mutex<Option<String>>> =
        StdArc::new(std::sync::Mutex::new(Some("dyn-token".to_string())));
    let token_slot_for_tool = token_slot.clone();

    let tool = HttpToolDefinition::builder(
        "get_org",
        "Fetch an org.",
        json!({"type": "object"}),
        format!("http://{addr}"),
        "/api/orgs/{org_id}",
    )
    .method(HttpMethod::Get)
    .auth(HttpAuthSource::Dynamic(Arc::new(move || {
        token_slot_for_tool.lock().unwrap().clone()
    })))
    .try_build()
    .expect("tool must build in tests");

    let _ = tool
        .execute(&ctx(), json!({"org_id": "o-1"}))
        .await
        .unwrap();

    let captured = cap.lock().await;
    let auth = captured
        .headers
        .iter()
        .find(|(n, _)| n == "authorization")
        .map(|(_, v)| v.clone())
        .expect("auth header");
    assert_eq!(auth, "Bearer dyn-token");
}

#[tokio::test]
async fn definition_reflects_configured_fields() {
    let tool = HttpToolDefinition::builder(
        "create_spec",
        "Draft a spec.",
        json!({"type": "object"}),
        "http://example",
        "/api/specs",
    )
    .method(HttpMethod::Post)
    .eager_input_streaming(true)
    .try_build()
    .expect("tool must build in tests");

    let def = tool.definition();
    assert_eq!(def.name, "create_spec");
    assert_eq!(def.description, "Draft a spec.");
    assert_eq!(def.eager_input_streaming, Some(true));
}

#[test]
fn path_encoding_handles_spaces_and_unicode() {
    assert_eq!(urlencode_path("a b"), "a%20b");
    assert_eq!(urlencode_path("π"), "%CF%80");
    assert_eq!(urlencode_path("safe-123._~"), "safe-123._~");
}
