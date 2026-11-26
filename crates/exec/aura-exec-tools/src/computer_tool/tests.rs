
use super::*;

use std::net::SocketAddr;

use crate::sandbox::Sandbox;
use crate::ToolConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn ctx() -> ToolContext {
    let dir = std::env::temp_dir();
    ToolContext::new(Sandbox::new(&dir).unwrap(), ToolConfig::default())
}

/// Minimal single-connection HTTP/1.1 mock returning a fixed JSON body.
async fn start_mock(status: u16, body: &'static str) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let Ok((mut sock, _)) = listener.accept().await else {
            return;
        };
        let mut buf = vec![0u8; 8 * 1024];
        let _ = sock.read(&mut buf).await.unwrap_or(0);
        let reason = if status == 200 { "OK" } else { "Error" };
        let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        let _ = sock.write_all(response.as_bytes()).await;
        let _ = sock.shutdown().await;
    });
    (addr, handle)
}

#[test]
fn build_body_clamps_coordinates() {
    let body = build_executor_body(&json!({"action": "left_click", "coordinate": [5000, -10]}))
        .expect("valid action");
    assert_eq!(body["x"], json!(DISPLAY_WIDTH_PX - 1));
    assert_eq!(body["y"], json!(0));
    assert_eq!(body["action"], json!("left_click"));
}

#[test]
fn build_body_maps_key_text_to_key_field() {
    let body =
        build_executor_body(&json!({"action": "key", "text": "ctrl+s"})).expect("valid action");
    assert_eq!(body["key"], json!("ctrl+s"));
    assert!(body.get("text").is_none());
}

#[test]
fn build_body_maps_type_text_to_text_field() {
    let body =
        build_executor_body(&json!({"action": "type", "text": "hello"})).expect("valid action");
    assert_eq!(body["text"], json!("hello"));
}

#[test]
fn build_body_translates_scroll_direction() {
    let body = build_executor_body(
        &json!({"action": "scroll", "scroll_direction": "up", "scroll_amount": 4}),
    )
    .expect("valid action");
    assert_eq!(body["dx"], json!(0));
    assert_eq!(body["dy"], json!(-4));
}

#[test]
fn build_body_rejects_unknown_action() {
    let err = build_executor_body(&json!({"action": "teleport"})).unwrap_err();
    assert!(matches!(err, ToolError::InvalidArguments(_)));
}

#[tokio::test]
async fn execute_screenshot_returns_image_result() {
    let (addr, _h) = start_mock(
        200,
        r#"{"ok":true,"image_base64":"aGVsbG8=","width":1280,"height":800}"#,
    )
    .await;
    let tool = ComputerTool::new(format!("http://{addr}"));
    let result = tool
        .execute(&ctx(), json!({"action": "screenshot"}))
        .await
        .expect("execute ok");
    assert!(result.ok);
    let image = result.image.expect("image attached");
    assert_eq!(image.base64, "aGVsbG8=");
    assert_eq!(image.media_type, SCREENSHOT_MEDIA_TYPE);
}

#[tokio::test]
async fn execute_executor_error_maps_to_failure() {
    let (addr, _h) = start_mock(200, r#"{"ok":false,"error":"display not found"}"#).await;
    let tool = ComputerTool::new(format!("http://{addr}"));
    let result = tool
        .execute(&ctx(), json!({"action": "screenshot"}))
        .await
        .expect("execute ok");
    assert!(!result.ok);
    assert!(result.image.is_none());
    assert_eq!(String::from_utf8_lossy(&result.stderr), "display not found");
}

#[tokio::test]
async fn execute_non_2xx_maps_to_failure() {
    let (addr, _h) = start_mock(500, r#"{"error":"boom"}"#).await;
    let tool = ComputerTool::new(format!("http://{addr}"));
    let result = tool
        .execute(&ctx(), json!({"action": "screenshot"}))
        .await
        .expect("execute ok");
    assert!(!result.ok);
}

#[test]
fn required_capability_is_computer_use() {
    let tool = ComputerTool::new("http://127.0.0.1:1");
    assert_eq!(tool.required_capabilities(), vec![Capability::ComputerUse]);
    assert_eq!(tool.name(), "computer");
}
