use super::api_types::{
    ApiContent, ApiImageSource, ApiMessage, ApiOutputConfig, ApiThinkingConfig, ApiTool,
    ApiToolChoice,
};
use crate::{
    ContentBlock, ImageSource, Message, ModelRequest, Role, ToolChoice, ToolDefinition,
    ToolResultContent,
};

/// Resolve extended thinking config for a given model.
///
/// Uses the caller-supplied config when present; otherwise auto-enables
/// thinking for capable models when the token budget is large enough.
fn supports_adaptive_thinking(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    model.starts_with("claude-opus-4-6")
        || model.starts_with("aura-claude-opus-4-6")
        || model.starts_with("claude-opus-4-7")
        || model.starts_with("aura-claude-opus-4-7")
        || model.starts_with("claude-sonnet-4-6")
        || model.starts_with("aura-claude-sonnet-4-6")
}

pub(super) fn resolve_thinking(request: &ModelRequest, model: &str) -> Option<ApiThinkingConfig> {
    let adaptive = supports_adaptive_thinking(model);

    if let Some(ref cfg) = request.thinking {
        return Some(ApiThinkingConfig {
            thinking_type: if adaptive {
                "adaptive".to_string()
            } else {
                "enabled".to_string()
            },
            budget_tokens: if adaptive {
                None
            } else {
                Some(cfg.budget_tokens)
            },
        });
    }

    let supports_thinking = model.contains("claude-3-7")
        || model.contains("claude-opus-4")
        || model.contains("claude-sonnet-4");

    if supports_thinking && request.max_tokens > 2048 {
        Some(ApiThinkingConfig {
            thinking_type: if adaptive {
                "adaptive".to_string()
            } else {
                "enabled".to_string()
            },
            budget_tokens: if adaptive {
                None
            } else {
                Some((request.max_tokens / 2).clamp(1024, 16000))
            },
        })
    } else {
        None
    }
}

pub(super) fn resolve_output_config(
    request: &ModelRequest,
    model: &str,
) -> Option<ApiOutputConfig> {
    let thinking = resolve_thinking(request, model)?;
    if thinking.thinking_type == "adaptive" {
        Some(ApiOutputConfig {
            effort: "high".to_string(),
        })
    } else {
        None
    }
}

/// Build the system block as a JSON array, optionally adding `cache_control`.
pub(super) fn build_system_block(
    system_prompt: &str,
    prompt_caching_enabled: bool,
) -> serde_json::Value {
    if prompt_caching_enabled {
        serde_json::json!([{
            "type": "text",
            "text": system_prompt,
            "cache_control": {"type": "ephemeral"}
        }])
    } else {
        serde_json::json!([{
            "type": "text",
            "text": system_prompt
        }])
    }
}

pub(super) fn convert_messages_to_api(
    messages: &[Message],
    prompt_caching_enabled: bool,
) -> Vec<ApiMessage> {
    let mut api_messages: Vec<ApiMessage> = messages
        .iter()
        .map(|msg| {
            let role = match msg.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };

            let content: Vec<ApiContent> = msg
                .content
                .iter()
                .map(|block| match block {
                    ContentBlock::Text { text } => ApiContent::Text {
                        text: text.clone(),
                        cache_control: None,
                    },
                    ContentBlock::ToolUse { id, name, input } => ApiContent::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    },
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        let content_text = match content {
                            ToolResultContent::Text(s) => s.clone(),
                            ToolResultContent::Json(v) => {
                                serde_json::to_string(v).unwrap_or_default()
                            }
                        };
                        ApiContent::ToolResult {
                            tool_use_id: tool_use_id.clone(),
                            content: content_text,
                            is_error: Some(*is_error),
                            cache_control: None,
                        }
                    }
                    ContentBlock::Thinking {
                        thinking,
                        signature,
                    } => ApiContent::Thinking {
                        thinking: thinking.clone(),
                        signature: signature.clone(),
                    },
                    ContentBlock::Image { source } => ApiContent::Image {
                        source: ApiImageSource {
                            source_type: source.source_type.clone(),
                            media_type: source.media_type.clone(),
                            data: source.data.clone(),
                        },
                    },
                })
                .collect();

            ApiMessage {
                role: role.to_string(),
                content,
            }
        })
        .collect();

    if prompt_caching_enabled {
        if let Some(last_user) = api_messages.iter_mut().rev().find(|m| m.role == "user") {
            if let Some(last_block) = last_user.content.last_mut() {
                let ephemeral = serde_json::json!({"type": "ephemeral"});
                match last_block {
                    ApiContent::Text { cache_control, .. }
                    | ApiContent::ToolResult { cache_control, .. } => {
                        *cache_control = Some(ephemeral);
                    }
                    _ => {}
                }
            }
        }
    }

    api_messages
}

pub(super) fn convert_tools_to_api(
    tools: &[ToolDefinition],
    prompt_caching_enabled: bool,
) -> Vec<ApiTool> {
    let has_any_cache_control = tools.iter().any(|t| t.cache_control.is_some());

    let mut api_tools: Vec<ApiTool> = tools
        .iter()
        .map(|tool| ApiTool {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
            cache_control: tool
                .cache_control
                .as_ref()
                .map(|cc| serde_json::json!({"type": cc.cache_type})),
            eager_input_streaming: tool.eager_input_streaming,
        })
        .collect();

    if prompt_caching_enabled && !has_any_cache_control {
        if let Some(last_tool) = api_tools.last_mut() {
            last_tool.cache_control = Some(serde_json::json!({"type": "ephemeral"}));
        }
    }

    api_tools
}

pub(super) fn convert_tool_choice(choice: &ToolChoice) -> Option<ApiToolChoice> {
    match choice {
        ToolChoice::Auto => Some(ApiToolChoice::Auto),
        ToolChoice::None => None,
        ToolChoice::Required => Some(ApiToolChoice::Any),
        ToolChoice::Tool { name } => Some(ApiToolChoice::Tool { name: name.clone() }),
    }
}

pub(super) fn convert_response_to_aura(content: &[ApiContent]) -> Message {
    let blocks: Vec<ContentBlock> = content
        .iter()
        .map(|c| match c {
            ApiContent::Text { text, .. } => ContentBlock::Text { text: text.clone() },
            ApiContent::Thinking {
                thinking,
                signature,
            } => ContentBlock::Thinking {
                thinking: thinking.clone(),
                signature: signature.clone(),
            },
            ApiContent::ToolUse { id, name, input } => ContentBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            },
            ApiContent::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            } => ContentBlock::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content: ToolResultContent::Text(content.clone()),
                is_error: is_error.unwrap_or(false),
            },
            ApiContent::Image { source } => ContentBlock::Image {
                source: ImageSource {
                    source_type: source.source_type.clone(),
                    media_type: source.media_type.clone(),
                    data: source.data.clone(),
                },
            },
        })
        .collect();

    Message {
        role: Role::Assistant,
        content: blocks,
    }
}
