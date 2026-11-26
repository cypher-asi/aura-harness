//! Timeouts, streaming deadlines and the buffered-streaming fallback for
//! proxy models outside the Anthropic family.

use super::support::*;
use crate::anthropic::{AnthropicConfig, AnthropicProvider};
use crate::{Message, ModelProvider, ModelRequest, ReasonerError, StopReason, StreamEvent};
use futures_util::StreamExt;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;

#[tokio::test]
async fn test_complete_timeout() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let _server = tokio::spawn(async move {
        loop {
            let Ok((_socket, _)) = listener.accept().await else {
                break;
            };
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });

    let mut config = proxy_config("test-model", addr);
    config.timeout_ms = 200;

    let provider = AnthropicProvider::new(config).unwrap();
    let request = proxy_request("test-model").try_build().unwrap();

    let result = provider.complete(request).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, ReasonerError::Timeout),
        "expected Timeout, got: {err:?}"
    );
}

/// Serve an SSE stream of ten `ping` events 40ms apart, then
/// `message_stop`, after checking a request arrived.
async fn serve_slow_pings() -> (SocketAddr, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ping = "event: ping\ndata: {}\n\n";
    let stop = "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0; 8192];
        let received = socket.read(&mut buf).await.unwrap();
        assert!(received > 0);
        let length = ping.len() * 10 + stop.len();
        socket
            .write_all(
                format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {length}\r\n\r\n"
        )
                .as_bytes(),
            )
            .await
            .unwrap();
        for _ in 0..10 {
            tokio::time::sleep(Duration::from_millis(40)).await;
            socket.write_all(ping.as_bytes()).await.unwrap();
        }
        socket.write_all(stop.as_bytes()).await.unwrap();
    });
    (addr, server)
}

#[tokio::test]
async fn streaming_activity_outlives_request_deadline() {
    let (addr, server) = serve_slow_pings().await;
    let mut config = AnthropicConfig::new("claude-test-model");
    config.base_url = format!("http://{addr}");
    config.timeout_ms = 200;
    config.max_retries = 0;
    let provider = AnthropicProvider::new(config).unwrap();
    let request = ModelRequest::builder("claude-test-model", "system")
        .message(Message::user("test"))
        .try_build()
        .unwrap();
    let started = std::time::Instant::now();
    let mut stream = provider.complete_streaming(request).await.unwrap();
    let mut pings = 0;
    let mut stopped = false;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            StreamEvent::Ping => pings += 1,
            StreamEvent::MessageStop => stopped = true,
            _ => {}
        }
    }
    server.await.unwrap();
    assert_eq!(pings, 10);
    assert!(stopped);
    assert!(started.elapsed() > Duration::from_millis(200));
}

#[tokio::test]
async fn test_proxy_openai_models_fall_back_to_buffered_streaming() {
    let body = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"proxy ok"}],"model":"aura-gpt-4.1","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#;
    let (addr, server) = serve_json_once(body).await;

    let provider = AnthropicProvider::new(proxy_config("aura-gpt-4.1", addr)).unwrap();
    let request = proxy_request("aura-gpt-4.1").try_build().unwrap();

    let stream = provider.complete_streaming(request).await.unwrap();
    let events = stream.collect::<Vec<_>>().await;

    let captured = server.await.unwrap();
    assert!(
        !captured.contains(r#""stream":true"#),
        "Buffered fallback should avoid Anthropic SSE requests.\nCaptured request:\n{captured}"
    );

    assert!(
        events.iter().any(|event| matches!(
            event.as_ref().unwrap(),
            StreamEvent::MessageStart { model, .. } if model == "aura-gpt-4.1"
        )),
        "expected a MessageStart with model aura-gpt-4.1 somewhere in the event stream"
    );
    assert!(events.iter().any(|event| matches!(
        event.as_ref().unwrap(),
        StreamEvent::TextDelta { text } if text == "proxy ok"
    )));
    assert!(events.iter().any(|event| matches!(
        event.as_ref().unwrap(),
        StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            output_tokens: 5,
            ..
        }
    )));
    assert!(matches!(
        events.last().unwrap().as_ref().unwrap(),
        StreamEvent::MessageStop
    ));
}

/// Answer the first connection with a `529` overload and the second with
/// a `200` JSON `body`; hand back both captured requests.
async fn serve_overloaded_then_json(
    body: &'static str,
) -> (SocketAddr, JoinHandle<(String, String)>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut first_socket, _) = listener.accept().await.unwrap();
        let first_request = read_request(&mut first_socket, 8192).await;
        let overloaded = "HTTP/1.1 529 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: 35\r\n\r\n{\"type\":\"error\",\"error\":\"overloaded\"}";
        first_socket.write_all(overloaded.as_bytes()).await.unwrap();

        let (mut second_socket, _) = listener.accept().await.unwrap();
        let second_request = read_request(&mut second_socket, 8192).await;
        second_socket
            .write_all(json_ok(body).as_bytes())
            .await
            .unwrap();

        (first_request, second_request)
    });
    (addr, server)
}

#[tokio::test]
async fn test_cross_family_proxy_fallback_buffers_streaming_and_omits_anthropic_headers() {
    let body = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"fallback ok"}],"model":"aura-gpt-5-4","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#;
    let (addr, server) = serve_overloaded_then_json(body).await;

    let mut config = proxy_config("claude-opus-4-6", addr);
    config.fallback_model = Some("aura-gpt-5-4".to_string());

    let provider = AnthropicProvider::new(config).unwrap();
    let request = proxy_request("claude-opus-4-6")
        .max_tokens(8192)
        .try_build()
        .unwrap();

    let stream = provider.complete_streaming(request).await.unwrap();
    let events = stream.collect::<Vec<_>>().await;

    let (first_request, second_request) = server.await.unwrap();
    assert!(
        first_request.contains(r#""stream":true"#),
        "Primary Anthropic request should still use SSE.\nCaptured request:\n{first_request}"
    );
    assert!(
        !second_request.contains(r#""stream":true"#),
        "Cross-family fallback should buffer instead of using Anthropic SSE.\nCaptured request:\n{second_request}"
    );
    assert!(
        !second_request.contains("anthropic-beta"),
        "Cross-family fallback should omit Anthropic beta headers.\nCaptured request:\n{second_request}"
    );
    assert!(
        !second_request.contains(r#""thinking":"#),
        "Cross-family fallback should omit Anthropic thinking config.\nCaptured request:\n{second_request}"
    );
    assert!(
        !second_request.contains("output_config"),
        "Cross-family fallback should omit Anthropic output config.\nCaptured request:\n{second_request}"
    );
    assert!(events.iter().any(|event| matches!(
        event.as_ref().unwrap(),
        StreamEvent::TextDelta { text } if text == "fallback ok"
    )));
}

#[tokio::test]
async fn test_proxy_hint_prefers_non_anthropic_family_over_model_heuristics_for_streaming() {
    let body = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"proxy ok"}],"model":"claude-opus-4-6","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#;
    let (addr, server) = serve_json_once(body).await;

    let provider = AnthropicProvider::new(proxy_config("claude-opus-4-6", addr)).unwrap();
    let request = proxy_request("claude-opus-4-6")
        .upstream_provider_family(Some("openai".to_string()))
        .try_build()
        .unwrap();

    let stream = provider.complete_streaming(request).await.unwrap();
    let events = stream.collect::<Vec<_>>().await;

    let captured = server.await.unwrap();
    assert!(
        !captured.contains(r#""stream":true"#),
        "Explicit non-Anthropic family hints should force buffered proxy streaming even for Claude model names.\nCaptured request:\n{captured}"
    );
    assert!(
        !captured.contains("anthropic-beta"),
        "Explicit non-Anthropic family hints should suppress Anthropic proxy headers even for Claude model names.\nCaptured request:\n{captured}"
    );
    assert!(
        !captured.contains(r#""thinking":"#),
        "Explicit non-Anthropic family hints should suppress Anthropic thinking config even for Claude model names.\nCaptured request:\n{captured}"
    );
    assert!(
        !captured.contains("output_config"),
        "Explicit non-Anthropic family hints should suppress Anthropic output config even for Claude model names.\nCaptured request:\n{captured}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event.as_ref().unwrap(),
            StreamEvent::MessageStart { model, .. } if model == "claude-opus-4-6"
        )),
        "expected a MessageStart with model claude-opus-4-6 somewhere in the event stream"
    );
}
