//! A request's `cache_salt` reaches the first prefix Anthropic (or an
//! OpenAI-family upstream behind the router) caches, so the two arms of a
//! `/compare` never share a prompt-cache entry.

use super::convert::{build_request_system, convert_tool_entries_to_api, system_salt};
use super::{AnthropicConfig, AnthropicProvider};
use crate::{Message, ModelProvider, ModelRequest, ToolDefinition};
use serde_json::{json, Value};

const MARKER_A: &str = "[cache scope: x:A]";
const MARKER_B: &str = "[cache scope: x:B]";

fn tool(name: &str) -> ToolDefinition {
    ToolDefinition::new(name, format!("Use {name}."), json!({"type": "object"}))
}

fn request(salt: Option<&str>, tools: &[&str], preamble: Option<&str>) -> ModelRequest {
    let mut request = ModelRequest::builder("aura-claude-sonnet-4-6", "Be brief.")
        .message(Message::user("hi"))
        .tools(tools.iter().map(|name| tool(name)).collect())
        .cache_salt(salt.map(str::to_string))
        .try_build()
        .unwrap();
    request.system_preamble = preamble.map(str::to_string);
    request
}

/// The serialized `tools` and `system` the provider sends for `request`,
/// with prompt caching on, in the order Anthropic caches them.
fn prefix(request: &ModelRequest, openai_family: bool) -> (Value, Value) {
    let tools = if request.tools.is_empty() {
        Value::Null
    } else {
        serde_json::to_value(convert_tool_entries_to_api(
            &request.tools,
            true,
            request.cache_salt.as_deref(),
        ))
        .unwrap()
    };
    let system = build_request_system(request, true, system_salt(request, openai_family))
        .unwrap_or(Value::Null);
    (tools, system)
}

fn arms(tools: &[&str], preamble: Option<&str>, openai_family: bool) -> [(Value, Value); 2] {
    [
        prefix(&request(Some("x:A"), tools, preamble), openai_family),
        prefix(&request(Some("x:B"), tools, preamble), openai_family),
    ]
}

#[test]
fn no_salt_sends_the_request_as_without_the_field() {
    for tools in [
        &[][..],
        &["read"][..],
        &["computer"][..],
        &["computer", "read"][..],
    ] {
        for preamble in [None, Some("You are Claude Code.")] {
            for openai_family in [false, true] {
                let mut unsalted = ModelRequest::builder("aura-claude-sonnet-4-6", "Be brief.")
                    .message(Message::user("hi"))
                    .tools(tools.iter().map(|name| tool(name)).collect())
                    .try_build()
                    .unwrap();
                unsalted.system_preamble = preamble.map(str::to_string);
                assert_eq!(
                    prefix(&request(None, tools, preamble), openai_family),
                    prefix(&unsalted, openai_family),
                    "tools {tools:?}, preamble {preamble:?}, openai {openai_family}",
                );
            }
        }
    }
}

#[test]
fn salt_closes_the_first_tool_which_precedes_the_first_breakpoint() {
    let [(tools_a, system_a), (tools_b, system_b)] = arms(&["read", "write"], None, false);

    assert_ne!(tools_a, tools_b);
    assert_eq!(
        tools_a[0]["description"],
        format!("Use read.\n\n{MARKER_A}")
    );
    assert_eq!(
        tools_b[0]["description"],
        format!("Use read.\n\n{MARKER_B}")
    );
    assert_eq!(tools_a[1]["description"], "Use write.");
    assert!(tools_a[1]["cache_control"].is_object(), "first breakpoint");
    // The tools already keep the arms apart; the system stays as it was.
    assert_eq!(system_a, system_b);
    assert_eq!(system_a, prefix(&request(None, &["read"], None), false).1);
}

#[test]
fn salt_skips_the_computer_tool_which_keeps_no_description() {
    let [(tools_a, _), (tools_b, _)] = arms(&["computer", "read"], None, false);

    assert_eq!(tools_a[0], tools_b[0]);
    assert_eq!(
        tools_a[1]["description"],
        format!("Use read.\n\n{MARKER_A}")
    );
    assert_ne!(tools_a, tools_b);
}

#[test]
fn salt_opens_the_system_when_only_the_computer_tool_is_sent() {
    let [(tools_a, system_a), (tools_b, system_b)] = arms(&["computer"], None, false);

    assert_eq!(tools_a, tools_b);
    assert_ne!(system_a, system_b);
    assert_eq!(system_a[0]["text"], format!("{MARKER_A}\n\nBe brief."));
    assert!(system_a[0]["cache_control"].is_object());
}

#[test]
fn salt_opens_the_system_without_tools() {
    let [(_, system_a), (_, system_b)] = arms(&[], None, false);

    assert_ne!(system_a, system_b);
    assert_eq!(system_a[0]["text"], format!("{MARKER_A}\n\nBe brief."));
    assert_eq!(system_b[0]["text"], format!("{MARKER_B}\n\nBe brief."));
    assert!(system_a[0]["cache_control"].is_object(), "first breakpoint");
}

#[test]
fn salt_alone_makes_the_system_of_a_request_with_none() {
    let salted = |salt: &str| {
        let mut request = request(Some(salt), &[], None);
        request.system = String::new();
        prefix(&request, false).1
    };

    assert_eq!(salted("x:A")[0]["text"], MARKER_A);
    assert_eq!(salted("x:B")[0]["text"], MARKER_B);
    assert!(salted("x:A")[0]["cache_control"].is_object());
}

#[test]
fn salt_follows_the_preamble_which_stays_first() {
    let preamble = Some("You are Claude Code.");
    let [(_, system_a), (_, system_b)] = arms(&[], preamble, false);

    assert_eq!(
        system_a[0],
        json!({"type": "text", "text": "You are Claude Code."})
    );
    assert_eq!(system_a[0], system_b[0]);
    assert_eq!(system_a[1]["text"], format!("{MARKER_A}\n\nBe brief."));
    assert_eq!(system_b[1]["text"], format!("{MARKER_B}\n\nBe brief."));
    assert!(system_a[1]["cache_control"].is_object(), "first breakpoint");

    let [(tools_a, system_a), (tools_b, system_b)] = arms(&["read"], preamble, false);
    assert_ne!(tools_a, tools_b);
    assert_eq!(system_a, system_b);
}

#[test]
fn openai_family_salts_the_system_even_with_tools() {
    let [(tools_a, system_a), (tools_b, system_b)] = arms(&["read"], None, true);

    assert_ne!(tools_a, tools_b);
    assert_ne!(system_a, system_b);
    assert_eq!(system_a[0]["text"], format!("{MARKER_A}\n\nBe brief."));
    assert_eq!(system_b[0]["text"], format!("{MARKER_B}\n\nBe brief."));
}

/// The JSON body of the one request `provider` sends for `request`.
async fn sent_body(model: &str, request: ModelRequest) -> Value {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut raw = Vec::new();
        let mut buf = vec![0u8; 65536];
        let body_start = loop {
            let n = socket.read(&mut buf).await.unwrap();
            raw.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&raw).to_ascii_lowercase();
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text[..end]
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .map_or(0, |v| v.trim().parse::<usize>().unwrap());
                if raw.len() >= end + 4 + length || n == 0 {
                    break end + 4;
                }
            }
        };
        let body = r#"{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"model":"m","stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#;
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        serde_json::from_slice::<Value>(&raw[body_start..]).unwrap()
    });
    let provider = AnthropicProvider::new(AnthropicConfig {
        base_url: format!("http://{addr}"),
        max_retries: 0,
        ..AnthropicConfig::new(model)
    })
    .unwrap();
    let mut request = request;
    request.model = crate::ModelName::from(model);
    request.auth_token = Some("jwt".to_string());
    provider.complete(request).await.unwrap();
    server.await.unwrap()
}

#[tokio::test]
async fn provider_sends_the_salt_in_tools_and_openai_system() {
    let claude = sent_body(
        "aura-claude-sonnet-4-6",
        request(Some("x:A"), &["read"], None),
    )
    .await;
    assert_eq!(
        claude["tools"][0]["description"],
        format!("Use read.\n\n{MARKER_A}")
    );
    assert_eq!(claude["system"][0]["text"], "Be brief.");

    let gpt = sent_body("aura-gpt-5-4", request(Some("x:B"), &["read"], None)).await;
    assert_eq!(
        gpt["tools"][0]["description"],
        format!("Use read.\n\n{MARKER_B}")
    );
    assert_eq!(gpt["system"][0]["text"], format!("{MARKER_B}\n\nBe brief."));
}
