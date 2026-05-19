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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ThinkingMode {
    Adaptive,
    Enabled,
}

fn normalize_anthropic_model(model: &str) -> String {
    model
        .trim()
        .to_ascii_lowercase()
        .trim_start_matches("aura-")
        .to_string()
}

fn thinking_mode_for_model(model: &str) -> Option<ThinkingMode> {
    let model = normalize_anthropic_model(model);
    if model.starts_with("claude-opus-4") || model.starts_with("claude-sonnet-4") {
        Some(ThinkingMode::Adaptive)
    } else if model.starts_with("claude-3-7-sonnet") {
        Some(ThinkingMode::Enabled)
    } else {
        None
    }
}

pub(super) fn resolve_thinking(request: &ModelRequest, model: &str) -> Option<ApiThinkingConfig> {
    let thinking_mode = thinking_mode_for_model(model)?;

    if let Some(ref cfg) = request.thinking {
        return Some(ApiThinkingConfig {
            thinking_type: match thinking_mode {
                ThinkingMode::Adaptive => "adaptive".to_string(),
                ThinkingMode::Enabled => "enabled".to_string(),
            },
            budget_tokens: match thinking_mode {
                ThinkingMode::Adaptive => None,
                ThinkingMode::Enabled => Some(cfg.budget_tokens),
            },
        });
    }

    if request.max_tokens.get() > 2048 {
        Some(ApiThinkingConfig {
            thinking_type: match thinking_mode {
                ThinkingMode::Adaptive => "adaptive".to_string(),
                ThinkingMode::Enabled => "enabled".to_string(),
            },
            budget_tokens: match thinking_mode {
                ThinkingMode::Adaptive => None,
                ThinkingMode::Enabled => Some((request.max_tokens.get() / 2).clamp(1024, 16000)),
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

    dedupe_tool_results(&mut api_messages);

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

/// Collapse duplicate `tool_result` blocks so the request honors Anthropic's
/// invariant that each `tool_use_id` may appear in at most one `tool_result`
/// block across the entire `messages[]` array.
///
/// Anthropic rejects the whole conversation with
/// `each tool_use must have a single result. Found multiple tool_result
/// blocks with id: <toolu_…>` when this rule is violated. Duplicates have
/// been observed slipping into the outbound queue from upstream recovery
/// paths (most notably `handle_max_tokens` synthesizing a placeholder for a
/// pending tool that later receives the real result), so this acts as a
/// last-line safety net before the body is serialized.
///
/// Semantics (mirrors `dedupe_tool_results_by_id` in `aura-os`'s
/// `compaction.rs`, but operates array-wide on typed [`ApiContent`]):
///
/// * **Last-write-wins on the body**: the kept block's `content`,
///   `is_error`, and `cache_control` come from the *last* occurrence of the
///   id, because that is the freshest observation.
/// * **Kept-in-place at the first occurrence**: the surviving block stays
///   at the position of the *first* occurrence, so the model still sees
///   results in the timeline order they were originally reported.
/// * **Empty messages are dropped**: if a message's only blocks were
///   duplicate `ToolResult`s, the now-empty message is removed because
///   Anthropic also 400s on empty `content` arrays.
/// * **Blocks without a `tool_use_id` pass through**: defensive guard for a
///   construction that shouldn't be reachable for `ToolResult`. We
///   deliberately do not silently drop these — the API rejecting them
///   loudly is the desired forensic signal.
/// * Non-`ToolResult` blocks and ids that appear exactly once are
///   untouched.
///
/// Emits one `tracing::warn!` per duplicated id (with the id and the count
/// of removed copies) when the sweep actually fires, so the upstream
/// emission path is easy to find in production logs.
pub(super) fn dedupe_tool_results(api_messages: &mut Vec<ApiMessage>) {
    use std::collections::{HashMap, HashSet};

    let mut positions_by_id: HashMap<String, Vec<(usize, usize)>> = HashMap::new();

    for (mi, msg) in api_messages.iter().enumerate() {
        for (ci, block) in msg.content.iter().enumerate() {
            if let ApiContent::ToolResult { tool_use_id, .. } = block {
                if tool_use_id.is_empty() {
                    continue;
                }
                positions_by_id
                    .entry(tool_use_id.clone())
                    .or_default()
                    .push((mi, ci));
            }
        }
    }

    let mut to_remove: HashSet<(usize, usize)> = HashSet::new();

    for (id, positions) in &positions_by_id {
        if positions.len() <= 1 {
            continue;
        }
        let copies_removed = positions.len() - 1;
        tracing::warn!(
            tool_use_id = %id,
            copies_removed = copies_removed,
            "convert_messages_to_api: deduplicated tool_result blocks before sending to Anthropic; \
             upstream emission path is likely synthesizing a placeholder that later collides with a real result"
        );

        let (first_mi, first_ci) = positions[0];
        let (last_mi, last_ci) = positions[positions.len() - 1];

        let (last_content, last_is_error, last_cache_control) =
            match &api_messages[last_mi].content[last_ci] {
                ApiContent::ToolResult {
                    content,
                    is_error,
                    cache_control,
                    ..
                } => (content.clone(), *is_error, cache_control.clone()),
                _ => continue,
            };

        if let ApiContent::ToolResult {
            content,
            is_error,
            cache_control,
            ..
        } = &mut api_messages[first_mi].content[first_ci]
        {
            *content = last_content;
            *is_error = last_is_error;
            *cache_control = last_cache_control;
        }

        for pos in positions.iter().skip(1) {
            to_remove.insert(*pos);
        }
    }

    if to_remove.is_empty() {
        return;
    }

    for (mi, msg) in api_messages.iter_mut().enumerate() {
        let mut indices_to_remove: Vec<usize> = to_remove
            .iter()
            .filter_map(|&(m, c)| if m == mi { Some(c) } else { None })
            .collect();
        indices_to_remove.sort_unstable_by(|a, b| b.cmp(a));
        for ci in indices_to_remove {
            msg.content.remove(ci);
        }
    }

    api_messages.retain(|m| !m.content.is_empty());
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
