//! `ModelRequest` → Responses API body for Sign in with ChatGPT.
//!
//! The ChatGPT preview rejects sampling and limit fields
//! (`max_output_tokens`, `temperature`, `top_p`, ...), requires
//! `store: false` with streaming, and only accepts function tools nested
//! inside a `namespace` tool.

use serde_json::{json, Map, Value};

use super::{reasoning_items, TOOL_NAMESPACE};
use crate::{ContentBlock, ModelRequest, Role, ToolChoice, ToolResultContent};

/// Build the JSON body for `POST /responses`.
pub(super) fn request_body(request: &ModelRequest) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), json!(request.model.as_str()));
    if !request.system.is_empty() {
        body.insert("instructions".into(), json!(request.system));
    }
    body.insert("input".into(), Value::Array(input_items(request)));
    if !request.tools.is_empty() {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                })
            })
            .collect();
        body.insert(
            "tools".into(),
            json!([{
                "type": "namespace",
                "name": TOOL_NAMESPACE,
                "description": "Aura harness tools",
                "tools": tools,
            }]),
        );
        body.insert("tool_choice".into(), tool_choice(&request.tool_choice));
        body.insert(
            "parallel_tool_calls".into(),
            json!(request.parallel_tool_use),
        );
    }
    body.insert("store".into(), json!(false));
    body.insert("stream".into(), json!(true));
    body.insert("include".into(), json!(["reasoning.encrypted_content"]));
    Value::Object(body)
}

fn tool_choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::None => json!("none"),
        // Naming a namespaced tool is not part of the preview contract;
        // forcing *a* call is the closest supported behaviour.
        ToolChoice::Required | ToolChoice::Tool { .. } => json!("required"),
    }
}

fn input_items(request: &ModelRequest) -> Vec<Value> {
    let mut items = Vec::new();
    for message in &request.messages {
        match message.role {
            Role::User => user_items(&message.content, &mut items),
            Role::Assistant => assistant_items(&message.content, &mut items),
        }
    }
    items
}

fn user_items(blocks: &[ContentBlock], items: &mut Vec<Value>) {
    let mut parts = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text } => parts.push(json!({"type": "input_text", "text": text})),
            ContentBlock::Image { source } => parts.push(json!({
                "type": "input_image",
                "image_url": format!("data:{};base64,{}", source.media_type, source.data),
            })),
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                flush_user(&mut parts, items);
                let text = match content {
                    ToolResultContent::Text(s) => s.clone(),
                    ToolResultContent::Json(v) => v.to_string(),
                };
                let output = if *is_error {
                    format!("Error: {text}")
                } else {
                    text
                };
                items.push(json!({
                    "type": "function_call_output",
                    "call_id": tool_use_id,
                    "output": output,
                }));
            }
            ContentBlock::Thinking { .. } | ContentBlock::ToolUse { .. } => {}
        }
    }
    flush_user(&mut parts, items);
}

fn flush_user(parts: &mut Vec<Value>, items: &mut Vec<Value>) {
    if !parts.is_empty() {
        items.push(json!({"role": "user", "content": std::mem::take(parts)}));
    }
}

fn assistant_items(blocks: &[ContentBlock], items: &mut Vec<Value>) {
    for block in blocks {
        match block {
            ContentBlock::Thinking { signature, .. } => {
                // Claude thinking blocks cannot be replayed to OpenAI.
                if let Some(replayed) = signature.as_deref().and_then(reasoning_items) {
                    items.extend(replayed);
                }
            }
            ContentBlock::Text { text } if !text.is_empty() => items.push(json!({
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": text}],
            })),
            ContentBlock::ToolUse { id, name, input } => items.push(json!({
                "type": "function_call",
                "namespace": TOOL_NAMESPACE,
                "call_id": id,
                "name": name,
                "arguments": input.to_string(),
            })),
            _ => {}
        }
    }
}
