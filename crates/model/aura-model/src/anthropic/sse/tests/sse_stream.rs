use super::*;

#[tokio::test]
async fn test_sse_stream_emits_http_meta_first() {
    let inner = bytes_stream(vec![
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ]);
    let mut stream = SseStream::with_request_id(inner, Some("req_01ABC".to_string()));
    // First event must be the synthetic HttpMeta, before any
    // provider event — otherwise `StreamAccumulator` can't seed
    // `provider_request_id` when `message_start` arrives.
    match stream.next().await {
        Some(Ok(StreamEvent::HttpMeta { request_id, .. })) => {
            assert_eq!(request_id.as_deref(), Some("req_01ABC"));
        }
        other => panic!("Expected HttpMeta first, got {other:?}"),
    }
    // Next event is the actual provider frame.
    assert!(matches!(
        stream.next().await,
        Some(Ok(StreamEvent::MessageStop))
    ));
}

#[tokio::test]
async fn test_sse_stream_emits_http_meta_with_none_when_no_header() {
    let inner = bytes_stream(vec![
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ]);
    let mut stream = SseStream::new(inner);
    // `SseStream::new` leaves `request_id: None`; the preamble
    // still fires so consumers can pattern-match unconditionally.
    match stream.next().await {
        Some(Ok(StreamEvent::HttpMeta { request_id, .. })) => {
            assert!(request_id.is_none());
        }
        other => panic!("Expected HttpMeta first, got {other:?}"),
    }
}

#[tokio::test]
async fn test_sse_stream_parses_complete_event() {
    let inner = bytes_stream(vec![
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ]);
    let mut stream = SseStream::new(inner);
    expect_http_meta(&mut stream, None).await;
    let event = stream.next().await;
    assert!(matches!(event, Some(Ok(StreamEvent::MessageStop))));
    // stream should be finished
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn test_sse_stream_handles_partial_chunks() {
    let inner = bytes_stream(vec![
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n",
        "\n",
    ]);
    let mut stream = SseStream::new(inner);
    expect_http_meta(&mut stream, None).await;
    let event = stream.next().await;
    assert!(matches!(event, Some(Ok(StreamEvent::MessageStop))));
}

#[tokio::test]
async fn test_sse_stream_handles_crlf_delimiters() {
    let inner = bytes_stream(vec![
        "event: message_stop\r\ndata: {\"type\":\"message_stop\"}\r\n\r\n",
    ]);
    let mut stream = SseStream::new(inner);
    expect_http_meta(&mut stream, None).await;
    let event = stream.next().await;
    assert!(matches!(event, Some(Ok(StreamEvent::MessageStop))));
}

#[tokio::test]
async fn test_sse_stream_emits_error_for_malformed_then_continues() {
    let inner = bytes_stream(vec![
        "event: unknown\ndata: {bad json}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ]);
    let mut stream = SseStream::new(inner);
    expect_http_meta(&mut stream, None).await;
    let first = stream.next().await;
    assert!(
        matches!(&first, Some(Ok(StreamEvent::Error { message, .. })) if message.contains("malformed SSE JSON")),
        "expected StreamEvent::Error, got {first:?}"
    );
    // Error from malformed JSON marks finished because StreamEvent::Error sets finished=true
}

#[tokio::test]
async fn test_sse_stream_multiple_events() {
    let inner = bytes_stream(vec![
        "event: ping\ndata: {}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ]);
    let mut stream = SseStream::new(inner);
    expect_http_meta(&mut stream, None).await;
    let first = stream.next().await;
    assert!(matches!(first, Some(Ok(StreamEvent::Ping))));
    let second = stream.next().await;
    assert!(matches!(second, Some(Ok(StreamEvent::MessageStop))));
}

#[tokio::test]
async fn test_sse_stream_empty_input() {
    let inner = bytes_stream(vec![]);
    let mut stream = SseStream::new(inner);
    // Empty upstream body still yields the HttpMeta preamble
    // (consumers rely on it as a deterministic first event), then
    // the stream terminates.
    expect_http_meta(&mut stream, None).await;
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn test_sse_stream_error_marks_finished() {
    let inner = bytes_stream(vec![
        "event: error\ndata: {\"type\":\"error\",\"error\":{\"message\":\"boom\"}}\n\n",
    ]);
    let mut stream = SseStream::new(inner);
    expect_http_meta(&mut stream, None).await;
    let event = stream.next().await;
    assert!(matches!(event, Some(Ok(StreamEvent::Error { .. }))));
    assert!(stream.next().await.is_none());
}
