//! Cross-provider reasoning filtering by `ReasoningOrigin`.

use crate::anthropic::api_types::{ApiContent, ApiMessage};
use crate::anthropic::convert::convert_messages_to_api;
use crate::anthropic::{AnthropicConfig, AnthropicProvider, ReasoningOrigin};
use crate::{
    ContentBlock, Message, ModelProvider, ModelRequest, ModelRequestKind, Role, ToolResultContent,
};
use futures_util::StreamExt;

fn thinking(text: &str, signature: Option<&str>) -> ContentBlock {
    ContentBlock::Thinking {
        thinking: text.to_string(),
        signature: signature.map(str::to_string),
    }
}

/// A turn holding reasoning from every provider, then a tool call.
fn mixed_reasoning_conversation() -> Vec<Message> {
    let openrouter_sig = format!("{}or-sig", crate::anthropic::OPENROUTER_REASONING_PREFIX);
    vec![
        Message::user("Read the file."),
        Message::new(
            Role::Assistant,
            vec![
                thinking("claude", Some("claude-sig")),
                thinking("openrouter", Some(&openrouter_sig)),
                thinking("unsigned", None),
                thinking("openai", Some("openai-responses:blob")),
                ContentBlock::ToolUse {
                    id: "toolu_1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({}),
                },
            ],
        ),
        Message::tool_results(vec![(
            "toolu_1".to_string(),
            ToolResultContent::Text("ok".to_string()),
            false,
        )]),
    ]
}

fn sent_thinking(messages: &[ApiMessage]) -> Vec<(String, Option<String>)> {
    messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|block| match block {
            ApiContent::Thinking {
                thinking,
                signature,
            } => Some((thinking.clone(), signature.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn each_reasoning_origin_receives_only_its_own_thinking() {
    let owned = |text: &str, sig: Option<&str>| (text.to_string(), sig.map(str::to_string));
    let cases = [
        (
            ReasoningOrigin::Anthropic,
            vec![owned("claude", Some("claude-sig"))],
        ),
        (
            ReasoningOrigin::Local,
            vec![owned("claude", Some("claude-sig")), owned("unsigned", None)],
        ),
        (
            ReasoningOrigin::OpenRouter,
            vec![owned("openrouter", Some("or-sig"))],
        ),
    ];
    for (origin, expected) in cases {
        let api = convert_messages_to_api(&mixed_reasoning_conversation(), false, origin);
        assert_eq!(sent_thinking(&api), expected, "origin {origin:?}");
        assert_eq!(api.len(), 3, "origin {origin:?}");
        assert!(
            matches!(api[1].content.last(), Some(ApiContent::ToolUse { id, .. }) if id == "toolu_1"),
            "origin {origin:?}"
        );
    }
}

#[test]
fn anthropic_origin_drops_empty_signatures() {
    let messages = vec![
        Message::user("hi"),
        Message::new(
            Role::Assistant,
            vec![thinking("empty", Some("")), ContentBlock::text("hello")],
        ),
    ];
    let api = convert_messages_to_api(&messages, false, ReasoningOrigin::Anthropic);
    assert!(sent_thinking(&api).is_empty());
    assert_eq!(api[1].content.len(), 1);
}

#[test]
fn assistant_turn_emptied_by_filtering_is_removed_when_alternation_survives() {
    let messages = vec![
        Message::user("hi"),
        Message::new(
            Role::Assistant,
            vec![thinking("openai", Some("openai-responses:x"))],
        ),
    ];
    let api = convert_messages_to_api(&messages, false, ReasoningOrigin::Anthropic);
    assert_eq!(api.len(), 1);
    assert_eq!(api[0].role, "user");
}

#[test]
fn assistant_turn_emptied_between_user_turns_keeps_a_placeholder() {
    let messages = vec![
        Message::user("hi"),
        Message::new(Role::Assistant, vec![thinking("unsigned", None)]),
        Message::user("again"),
    ];
    let api = convert_messages_to_api(&messages, false, ReasoningOrigin::Anthropic);
    let roles: Vec<&str> = api.iter().map(|m| m.role.as_str()).collect();
    assert_eq!(roles, ["user", "assistant", "user"]);
    assert!(matches!(
        api[1].content.as_slice(),
        [ApiContent::Text { text, .. }] if text == "(reasoning omitted)"
    ));
}

/// Serve one SSE `body` over raw TCP after draining the request.
async fn serve_sse_once(body: &'static str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut raw = Vec::new();
        let mut buf = vec![0u8; 65536];
        loop {
            let n = socket.read(&mut buf).await.unwrap();
            raw.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&raw).to_ascii_lowercase();
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text[..end]
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .map_or(0, |v| v.trim().parse::<usize>().unwrap());
                if raw.len() >= end + 4 + length || n == 0 {
                    break;
                }
            }
        }
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                     Content-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn openrouter_thinking_round_trips_only_to_openrouter() {
    let body = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-opus-4-7\",\"stop_reason\":null,\"usage\":{\"input_tokens\":3,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hmm\"}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig1\"}}\n\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    );
    let config = AnthropicConfig {
        base_url: serve_sse_once(body).await,
        max_retries: 0,
        ..AnthropicConfig::anthropic_direct()
    };
    let provider = AnthropicProvider::new(config)
        .unwrap()
        .with_sse_for_all_models()
        .without_router_body_extensions()
        .with_reasoning_origin(ReasoningOrigin::OpenRouter);
    let request = ModelRequest::builder("claude-opus-4-7", "system")
        .message(Message::user("hi"))
        .request_kind(ModelRequestKind::Chat)
        .try_build()
        .unwrap();
    let mut stream = provider.complete_streaming(request).await.unwrap();
    let mut accumulator = crate::StreamAccumulator::new();
    while let Some(event) = stream.next().await {
        accumulator.process(&event.unwrap());
    }
    let response = accumulator.into_response(0, 0).unwrap();
    let returned = response
        .message
        .content
        .iter()
        .find_map(|block| match block {
            ContentBlock::Thinking { signature, .. } => Some(signature.clone()),
            _ => None,
        });
    assert_eq!(returned, Some(Some("openrouter:sig1".to_string())));

    let conversation = vec![Message::user("hi"), response.message, Message::user("next")];
    let anthropic = convert_messages_to_api(&conversation, false, ReasoningOrigin::Anthropic);
    assert_eq!(sent_thinking(&anthropic), []);
    let openrouter = convert_messages_to_api(&conversation, false, ReasoningOrigin::OpenRouter);
    assert_eq!(
        sent_thinking(&openrouter),
        [("hmm".to_string(), Some("sig1".to_string()))]
    );
}
