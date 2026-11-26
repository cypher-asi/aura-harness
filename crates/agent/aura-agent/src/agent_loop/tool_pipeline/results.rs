//! Per-batch result reporting: console block, tracing, completion
//! events, and the trailing `tool_result` user message.

use std::collections::HashSet;

use aura_config::{TOOL_ERROR_PREVIEW_LIMIT, TOOL_RESULT_MAX_BYTES};
use aura_model::{ContentBlock, Message, Role, ToolResultContent};
use tokio::sync::mpsc::Sender;
use tracing::debug;

use crate::console;
use crate::dup_audit;
use crate::events::AgentLoopEvent;
use crate::helpers;
use crate::types::{ToolCallInfo, ToolCallResult};

use super::super::event_sink::emit as emit_event;
use super::super::tool_execution::truncate_preview;

/// Console + tracing + event emission for a finished tool batch.
///
/// Phase 4: merged from `tool_execution::emit_and_log_results` so
/// both Live and PreExecuted batches go through one log policy. The
/// pump path previously emitted ToolCallCompleted/ToolResult inline
/// in `handle_streamed_tool_use` without the surrounding
/// `console::tools_block` summary; the unified pipeline restores
/// that summary on both transports.
pub(super) fn emit_and_log_results(
    event_tx: Option<&Sender<AgentLoopEvent>>,
    tool_calls: &[ToolCallInfo],
    all_results: &[ToolCallResult],
    cached_ids: &HashSet<String>,
    blocked_ids: &HashSet<String>,
    reported: &HashSet<String>,
) {
    console::tools_block(tool_calls, all_results, cached_ids, blocked_ids);

    for r in all_results {
        log_tool_completion(tool_calls, r, cached_ids, blocked_ids);
    }

    for r in all_results
        .iter()
        .filter(|r| !reported.contains(&r.tool_use_id))
    {
        emit_tool_events(event_tx, tool_calls, r);
    }
}

fn log_tool_completion(
    tool_calls: &[ToolCallInfo],
    r: &ToolCallResult,
    cached_ids: &HashSet<String>,
    blocked_ids: &HashSet<String>,
) {
    let tool_name = tool_calls
        .iter()
        .find(|t| t.id == r.tool_use_id)
        .map_or("unknown", |t| t.name.as_str());
    let source = if cached_ids.contains(&r.tool_use_id) {
        "cache"
    } else if blocked_ids.contains(&r.tool_use_id) {
        "blocked"
    } else {
        "executor"
    };
    if r.is_error {
        let preview = truncate_preview(&r.content, TOOL_ERROR_PREVIEW_LIMIT);
        debug!(
            tool_use_id = %r.tool_use_id,
            tool_name = tool_name,
            is_write = helpers::is_write_tool(tool_name),
            is_error = r.is_error,
            stop_loop = r.stop_loop,
            source = source,
            result_len = r.content.len(),
            result_preview = preview.as_str(),
            "Tool call completed"
        );
    } else {
        debug!(
            tool_use_id = %r.tool_use_id,
            tool_name = tool_name,
            is_write = helpers::is_write_tool(tool_name),
            is_error = r.is_error,
            stop_loop = r.stop_loop,
            source = source,
            result_len = r.content.len(),
            "Tool call completed"
        );
    }
}

fn emit_tool_events(
    event_tx: Option<&Sender<AgentLoopEvent>>,
    tool_calls: &[ToolCallInfo],
    r: &ToolCallResult,
) {
    let info = tool_calls.iter().find(|t| t.id == r.tool_use_id);
    let tool_name = info.map_or_else(String::new, |t| t.name.clone());
    emit_event(
        event_tx,
        AgentLoopEvent::ToolCallCompleted {
            tool_use_id: r.tool_use_id.clone(),
            tool_name: tool_name.clone(),
            input: info.map_or(serde_json::Value::Null, |t| t.input.clone()),
            is_error: r.is_error,
        },
    );
    emit_event(
        event_tx,
        AgentLoopEvent::ToolResult {
            tool_use_id: r.tool_use_id.clone(),
            tool_name,
            content: r.content.clone(),
            is_error: r.is_error,
            image: r.image.clone(),
        },
    );
}

/// Push the trailing `Role::User` message carrying every
/// `tool_result` block (and any side-message text blocks).
///
/// Phase 4: absorbed from `tool_execution::push_tool_result_message_with_context`
/// so [`super::process_tool_results`] owns the entire dup_audit window.
/// Tool-result blocks are emitted before text blocks so the
/// Anthropic `tool_use ↔ tool_result` adjacency rule stays trivially
/// satisfied even when side messages (chunk-guard warnings,
/// auto-build output) ride along.
pub(in crate::agent_loop) fn push_tool_result_message(
    messages: &mut Vec<Message>,
    results: Vec<ToolCallResult>,
    context_texts: Vec<String>,
) {
    let mut blocks: Vec<ContentBlock> = Vec::new();
    // Images ride along as standalone `Image` blocks appended after the
    // `tool_result` blocks (Anthropic allows additional content after
    // tool_result blocks in the same user turn; text side-messages
    // already use this pattern below). The screenshot from a
    // computer-use action therefore reaches the model on the very next
    // iteration of the same turn. We deliberately do NOT nest the image
    // inside the tool_result content array here — aura-os's history
    // replay path reconstructs the nested form from the persisted
    // `image_base64`/`image_media_type` wire fields.
    let mut image_blocks: Vec<ContentBlock> = Vec::new();
    for r in results {
        if let Some(image) = &r.image {
            image_blocks.push(ContentBlock::Image {
                source: aura_model::ImageSource {
                    source_type: "base64".to_string(),
                    media_type: image.media_type.clone(),
                    data: image.base64.clone(),
                },
            });
        }
        blocks.push(ContentBlock::tool_result(
            &r.tool_use_id,
            ToolResultContent::text(cap_tool_result(r.content)),
            r.is_error,
        ));
    }
    blocks.append(&mut image_blocks);
    for text in context_texts {
        blocks.push(ContentBlock::Text { text });
    }

    if !blocks.is_empty() {
        dup_audit::audit_tool_result_duplicates(messages, "push_tool_result.pre");
        messages.push(Message::new(Role::User, blocks));
        dup_audit::audit_tool_result_duplicates(messages, "push_tool_result.post");
    }
}

/// `content` cut to [`TOOL_RESULT_MAX_BYTES`], keeping its start and end
/// around a marker that says how much is missing.
fn cap_tool_result(content: String) -> String {
    if content.len() <= TOOL_RESULT_MAX_BYTES || aura_config::agent().compaction.disabled {
        return content;
    }
    let mut head = TOOL_RESULT_MAX_BYTES / 2;
    while !content.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = content.len() - TOOL_RESULT_MAX_BYTES / 2;
    while !content.is_char_boundary(tail) {
        tail += 1;
    }
    format!(
        "{}\n\n[... {} of {} bytes cut from the middle: tool results over {} KB keep only \
         their start and end. Ask for a narrower slice, such as read_file with \
         start_line/end_line or a more specific search, to see the rest ...]\n\n{}",
        &content[..head],
        tail - head,
        content.len(),
        TOOL_RESULT_MAX_BYTES / 1024,
        &content[tail..],
    )
}
