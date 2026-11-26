//! Messages and content blocks, in both directions: the request's
//! conversation to the API's `messages`, and a response's content back to
//! a [`Message`].

use super::{
    cache_marker, dedupe_tool_results, drop_misplaced_tool_results, inject_missing_tool_results,
};
use crate::anthropic::api_types::{ApiContent, ApiImageSource, ApiMessage};
use crate::anthropic::{ReasoningOrigin, OPENROUTER_REASONING_PREFIX};
use crate::{ContentBlock, ImageSource, Message, Role, ToolResultContent};

pub(in crate::anthropic) fn convert_messages_to_api(
    messages: &[Message],
    prompt_caching_enabled: bool,
    origin: ReasoningOrigin,
) -> Vec<ApiMessage> {
    let converted: Vec<(ApiMessage, bool)> = messages
        .iter()
        .map(|msg| convert_message(msg, origin))
        .collect();
    let mut api_messages = repair_emptied_messages(converted);

    if prompt_caching_enabled {
        mark_cache_breakpoints(&mut api_messages);
    }

    dedupe_tool_results(&mut api_messages);
    drop_misplaced_tool_results(&mut api_messages);
    inject_missing_tool_results(&mut api_messages);

    api_messages
}

/// `msg` in the API's shape, and whether every block it had was filtered
/// out (see [`repair_emptied_messages`]).
fn convert_message(msg: &Message, origin: ReasoningOrigin) -> (ApiMessage, bool) {
    let role = match msg.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };

    let content: Vec<ApiContent> = msg
        .content
        .iter()
        .filter_map(|block| convert_block(block, origin))
        .collect();

    let emptied_by_filter = content.is_empty() && !msg.content.is_empty();
    (
        ApiMessage {
            role: role.to_string(),
            content,
        },
        emptied_by_filter,
    )
}

/// `block` in the API's shape, or `None` for reasoning `origin` must not see.
fn convert_block(block: &ContentBlock, origin: ReasoningOrigin) -> Option<ApiContent> {
    match block {
        ContentBlock::Text { text } => Some(ApiContent::Text {
            text: text.clone(),
            cache_control: None,
        }),
        ContentBlock::ToolUse { id, name, input } => Some(ApiContent::ToolUse {
            id: api_tool_id(id),
            name: name.clone(),
            input: input.clone(),
        }),
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            let content_text = match content {
                ToolResultContent::Text(s) => s.clone(),
                ToolResultContent::Json(v) => serde_json::to_string(v).unwrap_or_default(),
            };
            Some(ApiContent::ToolResult {
                tool_use_id: api_tool_id(tool_use_id),
                content: content_text,
                is_error: Some(*is_error),
                cache_control: None,
            })
        }
        ContentBlock::Thinking { .. } if crate::openai_responses::is_openai_reasoning(block) => {
            None
        }
        ContentBlock::Thinking {
            thinking,
            signature,
        } => replayable_signature(origin, signature.as_deref()).map(|signature| {
            ApiContent::Thinking {
                thinking: thinking.clone(),
                signature,
            }
        }),
        ContentBlock::Image { source } => Some(ApiContent::Image {
            source: ApiImageSource {
                source_type: source.source_type.clone(),
                media_type: source.media_type.clone(),
                data: source.data.clone(),
            },
        }),
    }
}

/// Breakpoints on the last two user messages (with the system
/// prompt and the tool list, four in all — the API maximum). The
/// API only looks about 20 blocks back from a breakpoint for an
/// earlier cache entry, so after a large batch of parallel tool
/// results the previous user message is what still matches.
fn mark_cache_breakpoints(api_messages: &mut [ApiMessage]) {
    for user in api_messages
        .iter_mut()
        .rev()
        .filter(|m| m.role == "user")
        .take(2)
    {
        if let Some(
            ApiContent::Text { cache_control, .. } | ApiContent::ToolResult { cache_control, .. },
        ) = user.content.last_mut()
        {
            *cache_control = Some(cache_marker());
        }
    }
}

/// Text sent in place of an assistant turn that held only foreign
/// reasoning, when removing the turn would break user/assistant
/// alternation. Anthropic rejects both empty content arrays and empty text.
const REASONING_OMITTED: &str = "(reasoning omitted)";

/// The signature to send for a (non-OpenAI) thinking block, or `None` when
/// `origin` must not see the block at all. The inner `Option` is the wire
/// signature, which may legitimately be absent for [`ReasoningOrigin::Local`].
fn replayable_signature(
    origin: ReasoningOrigin,
    signature: Option<&str>,
) -> Option<Option<String>> {
    let from_openrouter = signature.and_then(|sig| sig.strip_prefix(OPENROUTER_REASONING_PREFIX));
    match origin {
        // Anthropic verifies signatures and rejects unsigned thinking.
        ReasoningOrigin::Anthropic => signature
            .filter(|sig| !sig.is_empty() && from_openrouter.is_none())
            .map(|sig| Some(sig.to_owned())),
        ReasoningOrigin::Local => from_openrouter
            .is_none()
            .then(|| signature.map(str::to_owned)),
        ReasoningOrigin::OpenRouter => {
            from_openrouter.map(|sig| (!sig.is_empty()).then(|| sig.to_owned()))
        }
    }
}

/// Fix messages whose every block was filtered out as foreign reasoning
/// (`emptied` is true), since an empty content array is a 400.
///
/// Such a message is removed when its neighbours still alternate without it
/// (it is first, last, or sits between messages of different roles);
/// otherwise it keeps its slot with a single [`REASONING_OMITTED`] text
/// block. Messages that arrived empty are left as they are.
fn repair_emptied_messages(converted: Vec<(ApiMessage, bool)>) -> Vec<ApiMessage> {
    let mut repaired: Vec<ApiMessage> = Vec::with_capacity(converted.len());
    let mut remaining = converted.into_iter().peekable();
    while let Some((mut message, emptied)) = remaining.next() {
        if emptied {
            let previous = repaired.last().map(|m| m.role.as_str());
            let next = remaining.peek().map(|(m, _)| m.role.as_str());
            let removable = match (previous, next) {
                (None, None) => false,
                (previous, next) => previous != next,
            };
            if removable {
                tracing::debug!(
                    role = %message.role,
                    "convert_messages_to_api: removed message holding only foreign reasoning"
                );
                continue;
            }
            message.content.push(ApiContent::Text {
                text: REASONING_OMITTED.to_owned(),
                cache_control: None,
            });
        }
        repaired.push(message);
    }
    repaired
}

/// Map a tool id onto the `^[A-Za-z0-9_-]+$` alphabet Anthropic accepts,
/// replacing every other character with `_`. Applied to both `tool_use.id`
/// and `tool_result.tool_use_id`, so pairs keep matching; ids that are
/// already valid come back byte-identical. An empty id becomes
/// `"toolu_empty"`.
pub(in crate::anthropic) fn api_tool_id(id: &str) -> String {
    if id.is_empty() {
        return "toolu_empty".to_owned();
    }
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub(in crate::anthropic) fn convert_response_to_aura(content: &[ApiContent]) -> Message {
    let blocks: Vec<ContentBlock> = content.iter().filter_map(convert_response_block).collect();

    Message {
        role: Role::Assistant,
        content: blocks,
    }
}

fn convert_response_block(content: &ApiContent) -> Option<ContentBlock> {
    match content {
        ApiContent::Text { text, .. } => Some(ContentBlock::Text { text: text.clone() }),
        ApiContent::Thinking {
            thinking,
            signature,
        } => Some(ContentBlock::Thinking {
            thinking: thinking.clone(),
            signature: signature.clone(),
        }),
        // Nothing readable, and not replayed: dropped.
        ApiContent::RedactedThinking { .. } | ApiContent::Unknown => None,
        ApiContent::ToolUse { id, name, input } => Some(ContentBlock::ToolUse {
            id: id.clone(),
            name: name.clone(),
            input: input.clone(),
        }),
        ApiContent::ToolResult {
            tool_use_id,
            content,
            is_error,
            ..
        } => Some(ContentBlock::ToolResult {
            tool_use_id: tool_use_id.clone(),
            content: ToolResultContent::Text(content.clone()),
            is_error: is_error.unwrap_or(false),
        }),
        ApiContent::Image { source } => Some(ContentBlock::Image {
            source: ImageSource {
                source_type: source.source_type.clone(),
                media_type: source.media_type.clone(),
                data: source.data.clone(),
            },
        }),
    }
}
