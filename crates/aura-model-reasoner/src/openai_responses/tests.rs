use super::*;
use crate::{
    Message, ModelRequestKind, Role, StopReason, ToolChoice, ToolDefinition, ToolResultContent,
};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn tool() -> ToolDefinition {
    ToolDefinition {
        name: "read_file".into(),
        description: "Read a file".into(),
        input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        cache_control: None,
        eager_input_streaming: None,
    }
}

fn request(messages: Vec<Message>) -> ModelRequest {
    let mut builder = ModelRequest::builder("gpt-5.4", "be helpful")
        .request_kind(ModelRequestKind::Chat)
        .auth_token(Some("chatgpt-token".to_string()))
        .tools(vec![tool()]);
    for message in messages {
        builder = builder.message(message);
    }
    builder.try_build().expect("valid request")
}

fn reasoning_signature() -> String {
    encode_reasoning(&[json!({
        "type": "reasoning",
        "id": "rs_1",
        "summary": [],
        "encrypted_content": "opaque",
    })])
    .unwrap()
}

#[test]
fn body_follows_chatgpt_contract() {
    let assistant = Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::Thinking {
                thinking: "(reasoning)".into(),
                signature: Some(reasoning_signature()),
            },
            ContentBlock::Thinking {
                thinking: "claude thought".into(),
                signature: Some("anthropic-sig".into()),
            },
            ContentBlock::Text {
                text: "Reading.".into(),
            },
            ContentBlock::ToolUse {
                id: "call_1".into(),
                name: "read_file".into(),
                input: json!({"path": "a.rs"}),
            },
        ],
    };
    let results = Message {
        role: Role::User,
        content: vec![
            ContentBlock::ToolResult {
                tool_use_id: "call_1".into(),
                content: ToolResultContent::Text("fn main() {}".into()),
                is_error: false,
            },
            ContentBlock::Text {
                text: "thanks".into(),
            },
        ],
    };
    let mut req = request(vec![Message::user("read a.rs"), assistant, results]);
    req.tool_choice = ToolChoice::Auto;
    let body = convert::request_body(&req);

    assert_eq!(body["model"], "gpt-5.4");
    assert_eq!(body["instructions"], "be helpful");
    assert_eq!(body["store"], false);
    assert_eq!(body["stream"], true);
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    for rejected in [
        "max_output_tokens",
        "temperature",
        "top_p",
        "previous_response_id",
    ] {
        assert!(body.get(rejected).is_none(), "{rejected} must be omitted");
    }
    assert_eq!(body["tools"][0]["type"], "namespace");
    assert_eq!(body["tools"][0]["name"], TOOL_NAMESPACE);
    assert_eq!(body["tools"][0]["tools"][0]["parameters"]["type"], "object");

    let input = body["input"].as_array().unwrap();
    let kinds: Vec<&str> = input
        .iter()
        .map(|i| i.get("type").and_then(Value::as_str).unwrap_or("user"))
        .collect();
    assert_eq!(
        kinds,
        [
            "user",
            "reasoning",
            "message",
            "function_call",
            "function_call_output",
            "user"
        ]
    );
    assert_eq!(input[1]["encrypted_content"], "opaque");
    assert_eq!(input[3]["namespace"], TOOL_NAMESPACE);
    assert_eq!(input[3]["arguments"], r#"{"path":"a.rs"}"#);
    assert_eq!(input[4]["call_id"], "call_1");
    assert_eq!(input[4]["output"], "fn main() {}");
}

#[test]
fn reasoning_signatures_are_recognised() {
    let ours = ContentBlock::Thinking {
        thinking: String::new(),
        signature: Some(reasoning_signature()),
    };
    let theirs = ContentBlock::Thinking {
        thinking: String::new(),
        signature: Some("sig".into()),
    };
    assert!(is_openai_reasoning(&ours));
    assert!(!is_openai_reasoning(&theirs));
    assert_eq!(reasoning_items(&reasoning_signature()).unwrap().len(), 1);
}

fn sse(events: &[Value]) -> String {
    events
        .iter()
        .map(|e| {
            format!(
                "event: {}\r\ndata: {e}\r\n\r\n",
                e["type"].as_str().unwrap()
            )
        })
        .collect()
}

fn tool_turn_events() -> Vec<Value> {
    vec![
        json!({"type": "response.created", "response": {"id": "resp_1", "model": "gpt-5.4"}}),
        json!({"type": "response.output_item.done", "output_index": 0, "item": {
            "type": "reasoning", "id": "rs_1", "encrypted_content": "opaque",
            "summary": [{"type": "summary_text", "text": "Need the file."}]}}),
        json!({"type": "response.output_item.added", "output_index": 1, "item": {"type": "message"}}),
        json!({"type": "response.output_text.delta", "output_index": 1, "delta": "Let me "}),
        json!({"type": "response.output_text.delta", "output_index": 1, "delta": "look."}),
        json!({"type": "response.output_item.done", "output_index": 1, "item": {"type": "message"}}),
        json!({"type": "response.output_item.added", "output_index": 2, "item": {
            "type": "function_call", "call_id": "call_9", "name": "read_file", "namespace": "aura"}}),
        json!({"type": "response.function_call_arguments.delta", "output_index": 2, "delta": "{\"path\":"}),
        json!({"type": "response.function_call_arguments.delta", "output_index": 2, "delta": "\"a.rs\"}"}),
        json!({"type": "response.output_item.done", "output_index": 2, "item": {
            "type": "function_call", "call_id": "call_9", "name": "read_file", "arguments": "{\"path\":\"a.rs\"}"}}),
        json!({"type": "response.completed", "response": {"id": "resp_1", "model": "gpt-5.4",
            "usage": {"input_tokens": 120, "output_tokens": 30, "input_tokens_details": {"cached_tokens": 100}}}}),
    ]
}

fn accumulate(events: &[StreamEvent]) -> ModelResponse {
    let mut acc = StreamAccumulator::new();
    for event in events {
        acc.process(event);
    }
    acc.into_response(0, 1).expect("complete response")
}

#[test]
fn decoder_builds_text_tool_and_reasoning() {
    let mut decoder = Decoder::new("gpt-5.4");
    let events: Vec<StreamEvent> = tool_turn_events()
        .iter()
        .flat_map(|e| decoder.event(e))
        .collect();
    assert!(decoder.finished());
    let response = accumulate(&events);

    assert_eq!(response.stop_reason, StopReason::ToolUse);
    assert_eq!(response.usage.input_tokens, 120);
    assert_eq!(response.usage.output_tokens, 30);
    assert_eq!(response.usage.cache_read_input_tokens, Some(100));
    let blocks = &response.message.content;
    assert!(
        matches!(&blocks[0], ContentBlock::Thinking { thinking, .. } if thinking == "Need the file.")
    );
    assert!(is_openai_reasoning(&blocks[0]));
    assert!(matches!(&blocks[1], ContentBlock::Text { text } if text == "Let me look."));
    assert!(matches!(&blocks[2], ContentBlock::ToolUse { id, input, .. }
        if id == "call_9" && input == &json!({"path": "a.rs"})));
}

#[test]
fn json_response_object_is_accepted() {
    let body = json!({
        "id": "resp_2", "model": "gpt-5.4",
        "output": [{"type": "message", "content": [{"type": "output_text", "text": "hello"}]}],
        "usage": {"input_tokens": 5, "output_tokens": 1},
    });
    let response = accumulate(&Decoder::new("gpt-5.4").response_object(&body));
    assert_eq!(response.stop_reason, StopReason::EndTurn);
    assert_eq!(response.message.text_content(), "hello");
}

#[test]
fn failed_and_truncated_streams_surface_errors() {
    let mut decoder = Decoder::new("gpt-5.4");
    let events = decoder.event(&json!({"type": "response.failed",
        "response": {"error": {"message": "usage limit reached"}}}));
    assert!(
        matches!(&events[0], StreamEvent::Error { message, .. } if message == "usage limit reached")
    );

    let mut acc = StreamAccumulator::new();
    let mut decoder = Decoder::new("gpt-5.4");
    for event in &tool_turn_events()[..4] {
        for e in decoder.event(event) {
            acc.process(&e);
        }
    }
    assert!(!decoder.finished());
}

async fn serve_once(response: String) -> (std::net::SocketAddr, tokio::task::JoinHandle<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut raw = Vec::new();
        let mut buf = vec![0u8; 65536];
        loop {
            let n = socket.read(&mut buf).await.unwrap();
            raw.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&raw);
            if let Some(header_end) = text.find("\r\n\r\n") {
                let length = text[..header_end]
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if raw.len() >= header_end + 4 + length {
                    break;
                }
            }
        }
        socket.write_all(response.as_bytes()).await.unwrap();
        String::from_utf8_lossy(&raw).to_string()
    });
    (addr, handle)
}

#[tokio::test]
async fn streams_over_http_with_bearer() {
    let body = sse(&tool_turn_events());
    let (addr, server) = serve_once(format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nx-request-id: req_7\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    ))
    .await;
    let provider = OpenAiResponsesProvider::new(format!("http://{addr}/v1")).unwrap();
    let response = provider
        .complete(request(vec![Message::user("hi")]))
        .await
        .unwrap();
    assert_eq!(response.stop_reason, StopReason::ToolUse);
    assert_eq!(response.trace.provider_request_id.as_deref(), Some("req_7"));

    let raw = server.await.unwrap();
    let lower = raw.to_ascii_lowercase();
    assert!(lower.starts_with("post /v1/responses"));
    assert!(lower.contains("authorization: bearer chatgpt-token"));
    assert!(lower.contains("accept: text/event-stream"));
    assert!(raw.contains(r#""store":false"#));
}

#[tokio::test]
async fn http_errors_are_classified() {
    let body = r#"{"error":{"message":"token expired"}}"#;
    let (addr, _server) = serve_once(format!(
        "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    ))
    .await;
    let provider = OpenAiResponsesProvider::new(format!("http://{addr}/v1")).unwrap();
    let err = provider
        .complete(request(vec![Message::user("hi")]))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ReasonerError::Api { status: 401, ref message } if message.contains("token expired"))
    );
}
