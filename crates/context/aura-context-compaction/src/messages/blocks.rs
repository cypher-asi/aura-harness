//! Head/tail truncation of older content blocks and image stubbing.

use aura_model::{ContentBlock, Message, ToolResultContent};

use super::config::CompactionConfig;

/// Truncate a string to the given max chars, preserving head and tail.
///
/// `head_chars` and `tail_chars` control how many characters to keep from
/// the beginning and end respectively. Pass `None` to use 1/3 of `max_chars`.
#[must_use]
pub fn truncate_content(
    content: &str,
    max_chars: usize,
    head_chars: Option<usize>,
    tail_chars: Option<usize>,
) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }

    let mut head = head_chars.unwrap_or(max_chars / 3);
    let mut tail = tail_chars.unwrap_or(max_chars / 3);

    if head + tail > max_chars {
        let requested_total = head + tail;
        if requested_total == 0 {
            head = 0;
            tail = 0;
        } else {
            head = max_chars.saturating_mul(head) / requested_total;
            tail = max_chars.saturating_sub(head);
        }
    }

    let head = head.min(content.len());
    let tail = tail.min(content.len().saturating_sub(head));

    let head_part: String = content.chars().take(head).collect();
    let tail_part: String = content
        .chars()
        .rev()
        .take(tail)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    let omitted = content.len().saturating_sub(head + tail);
    format!("{head_part}\n\n[...content truncated ({omitted} chars omitted)...]\n\n{tail_part}")
}

/// Tool-result content rendered as text; JSON is serialized compactly.
pub(super) fn tool_result_text(content: &ToolResultContent) -> String {
    match content {
        ToolResultContent::Text(t) => t.clone(),
        ToolResultContent::Json(v) => serde_json::to_string(v).unwrap_or_default(),
    }
}

struct CompactionParams {
    compact_end: usize,
    head_chars: Option<usize>,
    tail_chars: Option<usize>,
}

const fn select_compaction_candidates(
    messages: &[Message],
    config: &CompactionConfig,
) -> Option<CompactionParams> {
    if messages.len() <= config.preserve_recent + 1 {
        return None;
    }
    let compact_end = messages.len().saturating_sub(config.preserve_recent);
    let is_emergency = config.preserve_recent == CompactionConfig::emergency().preserve_recent
        && config.tool_result_max_chars == CompactionConfig::emergency().tool_result_max_chars;
    let (head_chars, tail_chars) = if is_emergency {
        (Some(6000_usize), Some(3000_usize))
    } else {
        (None, None)
    };
    Some(CompactionParams {
        compact_end,
        head_chars,
        tail_chars,
    })
}

fn compact_content_block(
    block: &mut ContentBlock,
    config: &CompactionConfig,
    head_chars: Option<usize>,
    tail_chars: Option<usize>,
) {
    match block {
        ContentBlock::ToolResult { content, .. } => {
            let text = tool_result_text(content);
            if text.len() > config.tool_result_max_chars {
                *content = ToolResultContent::Text(truncate_content(
                    &text,
                    config.tool_result_max_chars,
                    head_chars,
                    tail_chars,
                ));
            }
        }
        ContentBlock::Text { text } => {
            if text.len() > config.text_max_chars {
                *text = truncate_content(text, config.text_max_chars, head_chars, tail_chars);
            }
        }
        _ => {}
    }
}

/// Replace an image block with a small text stub. Base64 image
/// payloads are by far the largest blocks in a transcript (hundreds of
/// KB each) and the model has already consumed them on the turn they
/// were attached; keeping them in older history permanently inflates
/// every subsequent request, to the point of tripping the wire-level
/// emergency body cap on each call.
fn stub_image_block(block: &mut ContentBlock) {
    if let ContentBlock::Image { source } = block {
        let kb = source.data.len() / 1024;
        let media_type = source.media_type.clone();
        *block = ContentBlock::Text {
            text: format!(
                "[image removed during context compaction: {media_type}, ~{kb} KB base64]"
            ),
        };
    }
}

/// Compact older messages using the given tier configuration.
pub fn compact_older_messages(messages: &mut [Message], config: &CompactionConfig) {
    let Some(params) = select_compaction_candidates(messages, config) else {
        return;
    };
    // Image stubbing covers messages[0] too: the first user turn is
    // exempt from text truncation (its task statement must survive
    // verbatim) but attached images there are the dominant payload and
    // must not be exempt with it.
    for msg in &mut messages[..params.compact_end] {
        for block in &mut msg.content {
            stub_image_block(block);
        }
    }
    for msg in &mut messages[1..params.compact_end] {
        for block in &mut msg.content {
            compact_content_block(block, config, params.head_chars, params.tail_chars);
        }
    }
}
