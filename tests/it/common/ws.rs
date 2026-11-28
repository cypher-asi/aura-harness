//! `WsClient`: a thin JSON-frame WebSocket client for `/stream/:run_id`.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message as WsMsg;

use super::auth::E2E_TEST_BEARER;

pub struct WsClient {
    pub write: futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        WsMsg,
    >,
    pub read: futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
}

impl WsClient {
    /// Open a raw WebSocket to `ws_url`, attaching the default test
    /// Bearer header. Used by tests that need an explicit URL — most
    /// callers should instead use [`open_chat_run`] which handles the
    /// `POST /v1/run` + `/stream/:run_id` attach in one step.
    pub async fn connect(ws_url: &str) -> Self {
        Self::connect_with_auth(ws_url, E2E_TEST_BEARER).await
    }

    /// Open a WebSocket without an Authorization header. Only used by
    /// tests that deliberately verify the 401 rejection path.
    pub async fn connect_anonymous(ws_url: &str) -> Self {
        let (stream, _) = tokio_tungstenite::connect_async(ws_url)
            .await
            .expect("ws connect");
        let (write, read) = stream.split();
        Self { write, read }
    }

    pub async fn connect_with_auth(ws_url: &str, bearer_token: &str) -> Self {
        use tokio_tungstenite::tungstenite::http::Request;
        let req = Request::builder()
            .uri(ws_url)
            .header("Authorization", format!("Bearer {bearer_token}"))
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tokio_tungstenite::tungstenite::handshake::client::generate_key(),
            )
            .header("Host", "localhost")
            .body(())
            .unwrap();
        let (stream, _) = tokio_tungstenite::connect_async(req)
            .await
            .expect("ws connect with auth");
        let (write, read) = stream.split();
        Self { write, read }
    }

    pub async fn send_json(&mut self, msg: &Value) {
        let text = serde_json::to_string(msg).unwrap();
        self.write
            .send(WsMsg::Text(text.into()))
            .await
            .expect("ws send");
    }

    pub async fn send_user_message(&mut self, content: &str) {
        self.send_json(&json!({"type": "user_message", "content": content}))
            .await;
    }

    pub async fn send_cancel(&mut self) {
        self.send_json(&json!({"type": "cancel"})).await;
    }

    pub async fn send_raw(&mut self, raw: &str) {
        self.write
            .send(WsMsg::Text(raw.to_string().into()))
            .await
            .expect("ws send raw");
    }

    pub async fn recv_json(&mut self) -> Option<Value> {
        self.recv_json_timeout(Duration::from_secs(30)).await
    }

    pub async fn recv_json_timeout(&mut self, timeout: Duration) -> Option<Value> {
        match tokio::time::timeout(timeout, self.read.next()).await {
            Ok(Some(Ok(WsMsg::Text(text)))) => serde_json::from_str(text.as_ref()).ok(),
            _ => None,
        }
    }

    pub async fn expect_session_ready(&mut self) -> Value {
        let msg = self.recv_json().await.expect("expected session_ready");
        assert_eq!(
            msg["type"], "session_ready",
            "expected session_ready, got: {msg}"
        );
        msg
    }

    /// Collect all messages for one turn until `assistant_message_end` or timeout.
    pub async fn collect_turn(&mut self, timeout: Duration) -> Vec<Value> {
        let mut messages = Vec::new();
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match self.recv_json_timeout(remaining).await {
                Some(msg) => {
                    let is_end = msg["type"] == "assistant_message_end";
                    let is_error =
                        msg["type"] == "error" && msg["recoverable"].as_bool() == Some(false);
                    messages.push(msg);
                    if is_end || is_error {
                        break;
                    }
                }
                None => break,
            }
        }
        messages
    }
}
