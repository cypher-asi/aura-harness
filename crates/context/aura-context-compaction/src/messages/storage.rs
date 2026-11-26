//! Storage-side caps on tool blobs kept in session history.

use aura_model::{ContentBlock, Message, ToolResultContent};
use serde_json::Value;

use super::redaction::RedactionMarker;

/// Hard upper bound on bytes-per-tool-blob kept in session storage.
pub const SESSION_TOOL_BLOB_MAX_BYTES: usize = 8 * 1024;

/// Cap each `ToolUse` input / `ToolResult` content in `messages` at the storage limit.
///
/// Invariant: `tool_use.input` MUST remain a JSON object on the wire — the
/// Anthropic Messages API rejects anything else with 400
/// `messages.N.content.M.tool_use.input: Input should be an object`.
/// Oversized inputs are redacted in-place via `RedactionMarker`, which drops
/// the largest top-level string field(s) and records a structured
/// `_redacted` summary, preserving the object shape.
pub fn truncate_messages_for_storage(messages: &mut [Message]) {
    fn truncate_str(s: &str, max: usize) -> Option<String> {
        if s.len() <= max {
            return None;
        }
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        Some(format!("{}... [truncated {} bytes]", &s[..end], s.len()))
    }

    for msg in messages.iter_mut() {
        for block in msg.content.iter_mut() {
            match block {
                ContentBlock::ToolUse { input, .. } => {
                    redact_oversized_tool_use_input(input, SESSION_TOOL_BLOB_MAX_BYTES);
                }
                ContentBlock::ToolResult { content, .. } => match content {
                    ToolResultContent::Text(t) => {
                        if let Some(truncated) = truncate_str(t, SESSION_TOOL_BLOB_MAX_BYTES) {
                            *t = truncated;
                        }
                    }
                    ToolResultContent::Json(v) => {
                        if let Ok(serialized) = serde_json::to_string(v) {
                            if let Some(truncated) =
                                truncate_str(&serialized, SESSION_TOOL_BLOB_MAX_BYTES)
                            {
                                *content = ToolResultContent::Text(truncated);
                            }
                        }
                    }
                },
                _ => {}
            }
        }
    }
}

/// Shrink an oversized `tool_use.input` in place while keeping it a JSON
/// object. Walks the top-level string fields in descending size order and
/// replaces each with a `RedactionMarker` summary until the serialized size
/// fits under `cap`. Non-object inputs and inputs already carrying a
/// redaction marker are left untouched: the former are invalid upstream
/// (and surfaced by the aura-os persistence guard), the latter are already
/// minimized.
fn redact_oversized_tool_use_input(input: &mut Value, cap: usize) {
    if !input.is_object() {
        return;
    }
    if RedactionMarker::is_marker(input) {
        return;
    }
    let Ok(mut serialized_len) = serde_json::to_string(input).map(|s| s.len()) else {
        return;
    };
    while serialized_len > cap {
        let largest = input.as_object().and_then(|m| {
            m.iter()
                .filter(|(k, v)| {
                    k.as_str() != "_redacted" && v.as_str().is_some_and(|s| !s.is_empty())
                })
                .max_by_key(|(_, v)| v.as_str().map_or(0, str::len))
                .map(|(k, v)| (k.clone(), v.as_str().map_or(0, str::len)))
        });
        let Some((field, bytes)) = largest else {
            return;
        };
        *input = RedactionMarker::mark(input, &field, bytes);
        let Ok(new_len) = serde_json::to_string(input).map(|s| s.len()) else {
            return;
        };
        if new_len >= serialized_len {
            return;
        }
        serialized_len = new_len;
    }
}

/// Public storage compaction API used by runtime.
pub fn compact_for_storage(messages: &mut [Message]) {
    truncate_messages_for_storage(messages);
}
