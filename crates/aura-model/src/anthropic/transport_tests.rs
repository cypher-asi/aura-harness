//! Real HTTP faults exercise reqwest's error classification and request retry boundary.
use super::{AnthropicConfig, AnthropicProvider};
use crate::{
    ContentBlock, Message, ModelProvider, ModelRequest, ReasonerError, StreamEvent,
    ToolResultContent,
};
use futures_util::StreamExt;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const RESPONSE: &str = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"recovered"}],"model":"grok-test","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#;

enum Reply {
    Disconnect,
    DelayedDisconnect,
    CompleteStream,
    TruncatedBody,
    Http(u16, &'static str),
    PartialStream,
}

struct FaultServer {
    url: String,
    requests: Arc<Mutex<Vec<serde_json::Value>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FaultServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FaultServer {
    async fn start(replies: Vec<Reply>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let task = tokio::spawn(async move {
            let mut replies = replies.into_iter();
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let body_start = loop {
                    let mut chunk = [0; 4096];
                    let n = socket.read(&mut chunk).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        break end + 4;
                    }
                };
                let headers = String::from_utf8_lossy(&bytes[..body_start]);
                let len: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap();
                while bytes.len() < body_start + len {
                    let mut chunk = [0; 4096];
                    let n = socket.read(&mut chunk).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&chunk[..n]);
                }
                captured
                    .lock()
                    .unwrap()
                    .push(serde_json::from_slice(&bytes[body_start..body_start + len]).unwrap());
                let response = match replies.next().unwrap_or(Reply::Disconnect) {
                    Reply::Disconnect => continue,
                    Reply::DelayedDisconnect => {
                        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
                        continue;
                    }
                    Reply::CompleteStream => {
                        let body = concat!(
                            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_ok\",\"model\":\"claude-test\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
                            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"recovered\"}}\n\n",
                            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
                        );
                        format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
                    },
                    Reply::TruncatedBody => "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 9999\r\nConnection: close\r\n\r\n{\"id\":\"partial".to_string(),
                    Reply::Http(status, body) => format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()),
                    Reply::PartialStream => concat!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 9999\r\nConnection: close\r\n\r\n",
                        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_partial\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-test\",\"content\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
                        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"partial output\"}}\n\n"
                    ).to_string(),
                };
                socket.write_all(response.as_bytes()).await.unwrap();
                socket.shutdown().await.unwrap();
            }
        });
        Self {
            url,
            requests,
            task,
        }
    }

    fn provider(&self, model: &str, max_retries: u32) -> AnthropicProvider {
        AnthropicProvider::new(AnthropicConfig {
            default_model: model.to_string(),
            timeout_ms: 1000,
            max_retries,
            backoff_initial_ms: 1,
            backoff_cap_ms: 2,
            min_request_interval_ms: 0,
            base_url: self.url.clone(),
            fallback_model: None,
            prompt_caching_enabled: false,
            emergency_body_cap_bytes: 0,
            cloudflare_max_retries: 0,
        })
        .unwrap()
    }
}

fn continuation_request(model: &str) -> ModelRequest {
    ModelRequest::builder(model, "Continue using the completed tool results.")
        .message(Message::user("inspect files then summarize"))
        .message(Message {
            role: crate::Role::Assistant,
            content: vec![ContentBlock::tool_use(
                "read_1",
                "fs.read",
                serde_json::json!({"path":"file.txt"}),
            )],
        })
        .message(Message::tool_results(vec![(
            "read_1".into(),
            ToolResultContent::Text("already read file".into()),
            false,
        )]))
        .auth_token(Some("test-token".to_string()))
        .try_build()
        .unwrap()
}

#[tokio::test]
async fn transport_disconnect_retries_buffered_grok_continuation_without_replaying_history() {
    let server = FaultServer::start(vec![Reply::Disconnect, Reply::Http(200, RESPONSE)]).await;
    let observations = Arc::new(Mutex::new(Vec::new()));
    let observed = observations.clone();
    let observer: crate::RetryObserver = Arc::new(move |info| observed.lock().unwrap().push(info));
    let provider = server.provider("grok-test", 2);
    let result = crate::DEBUG_RETRY_OBSERVER
        .scope(
            observer,
            provider.complete_streaming(continuation_request("grok-test")),
        )
        .await;
    assert!(
        result.is_ok(),
        "connection close before headers should recover: {:?}",
        result.err()
    );
    let events = result.unwrap().collect::<Vec<_>>().await;
    assert!(events
        .iter()
        .any(|event| matches!(event, Ok(StreamEvent::TextDelta { text }) if text == "recovered")));
    let observations = observations.lock().unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].reason, "transport");
    assert_eq!(observations[0].attempt, 2);
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0], requests[1],
        "retry must reuse exactly the pending model request"
    );
    assert_eq!(requests[1]["messages"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn transport_truncated_buffered_body_retries() {
    let server = FaultServer::start(vec![Reply::TruncatedBody, Reply::Http(200, RESPONSE)]).await;
    let result = server
        .provider("grok-test", 2)
        .complete(continuation_request("grok-test"))
        .await;
    assert!(
        result.is_ok(),
        "incomplete buffered HTTP body should recover: {result:?}"
    );
    assert_eq!(server.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn transport_disconnect_stops_at_configured_retry_budget() {
    let server = FaultServer::start(vec![]).await;
    let result = server
        .provider("grok-test", 2)
        .complete_streaming(continuation_request("grok-test"))
        .await;
    assert!(matches!(result, Err(ReasonerError::Request(_))));
    assert_eq!(
        server.requests.lock().unwrap().len(),
        3,
        "one initial attempt plus two retries, even for buffered streaming"
    );
}

#[tokio::test]
async fn transport_permanent_responses_are_not_retried() {
    for (status, body) in [
        (400, "bad request"),
        (401, "unauthorized"),
        (402, "no credits"),
        (403, "forbidden"),
        (200, "invalid JSON"),
    ] {
        let server =
            FaultServer::start(vec![Reply::Http(status, body), Reply::Http(200, RESPONSE)]).await;
        let result = server
            .provider("grok-test", 2)
            .complete(continuation_request("grok-test"))
            .await;
        assert!(result.is_err());
        assert_eq!(
            server.requests.lock().unwrap().len(),
            1,
            "must not retry status {status} or complete invalid JSON"
        );
    }
}

#[tokio::test]
async fn transport_partial_sse_is_propagated_without_provider_replay() {
    let server = FaultServer::start(vec![Reply::PartialStream, Reply::Http(200, RESPONSE)]).await;
    let mut stream = server
        .provider("claude-test", 2)
        .complete_streaming(continuation_request("claude-test"))
        .await
        .unwrap();
    let mut saw_partial = false;
    let mut saw_error = false;
    while let Some(event) = stream.next().await {
        match event {
            Ok(StreamEvent::TextDelta { .. }) => saw_partial = true,
            Ok(StreamEvent::Error { .. }) | Err(_) => saw_error = true,
            _ => {}
        }
    }
    assert!(saw_partial && saw_error);
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn transport_disconnect_before_native_sse_retries() {
    let server = FaultServer::start(vec![Reply::Disconnect, Reply::CompleteStream]).await;
    let stream = server
        .provider("claude-test", 2)
        .complete_streaming(continuation_request("claude-test"))
        .await
        .unwrap();
    let events = stream.collect::<Vec<_>>().await;
    assert!(events
        .iter()
        .any(|event| matches!(event, Ok(StreamEvent::TextDelta { text }) if text == "recovered")));
    assert!(events
        .iter()
        .any(|event| matches!(event, Ok(StreamEvent::MessageStop))));
    assert_eq!(server.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn transport_timeout_before_headers_retries() {
    let server =
        FaultServer::start(vec![Reply::DelayedDisconnect, Reply::Http(200, RESPONSE)]).await;
    let provider = AnthropicProvider::new(AnthropicConfig {
        timeout_ms: 250,
        ..server.provider("grok-test", 2).config
    })
    .unwrap();
    let result = provider.complete(continuation_request("grok-test")).await;
    assert!(
        result.is_ok(),
        "timeout before response must recover: {result:?}"
    );
    assert_eq!(server.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn transport_retry_can_be_disabled() {
    let server = FaultServer::start(vec![Reply::Disconnect, Reply::Http(200, RESPONSE)]).await;
    let result = server
        .provider("grok-test", 0)
        .complete(continuation_request("grok-test"))
        .await;
    assert!(matches!(result, Err(ReasonerError::Request(_))));
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}
