use super::{TRUNCATION_MARKER_BUDGET, TRUNCATION_MARKER_PREFIX};
use crate::anthropic::provider::waf::serialize_request_body;

/// Build a synthetic single-user-message body whose only content is
/// the truncation marker plus the most useful recoverable user context.
/// Used as the final fallback when no amount of history-trimming can
/// fit under the cap (e.g. one absolutely enormous single user
/// message). System messages from the original body are preserved
/// verbatim.
///
/// A tool loop's latest user-role message commonly contains only a
/// `tool_result`. Looking exclusively for a `text` block in that message
/// produced an `original_len=0,kept=0` marker, causing the model to report
/// that the user's prompt was empty. Prefer a real text block on the latest
/// turn; otherwise retain the latest human instruction and the current tool
/// result as plain text so the collapsed request still has actionable
/// context without violating Anthropic's tool-use pairing rules.
pub(in crate::anthropic::provider) fn collapse_messages_to_marker(
    body_bytes: &[u8],
    cap_bytes: usize,
) -> Vec<u8> {
    let parsed: serde_json::Value = match serde_json::from_slice(body_bytes) {
        Ok(v) => v,
        Err(_) => return body_bytes.to_vec(),
    };
    let mut value = parsed;

    let mut salvaged_user_context: Option<String> = None;
    let mut system_messages: Vec<serde_json::Value> = Vec::new();
    if let Some(messages) = value.get("messages").and_then(serde_json::Value::as_array) {
        for m in messages {
            if m.get("role").and_then(serde_json::Value::as_str) == Some("system") {
                system_messages.push(m.clone());
            }
        }
        if let Some(last_user) = messages
            .iter()
            .rev()
            .find(|m| m.get("role").and_then(serde_json::Value::as_str) == Some("user"))
        {
            if let Some(blocks) = last_user
                .get("content")
                .and_then(serde_json::Value::as_array)
            {
                if let Some(text) = latest_non_empty_text_block(blocks) {
                    salvaged_user_context = Some(text.to_string());
                } else if let Some(tool_result) = largest_non_empty_tool_result(blocks) {
                    let latest_instruction = messages
                        .iter()
                        .rev()
                        .filter(|m| {
                            m.get("role").and_then(serde_json::Value::as_str) == Some("user")
                        })
                        .filter_map(|m| {
                            m.get("content")
                                .and_then(serde_json::Value::as_array)
                                .and_then(|blocks| latest_non_empty_text_block(blocks))
                        })
                        .next();

                    salvaged_user_context = Some(match latest_instruction {
                        Some(instruction) => format!(
                            "[latest user instruction retained after history truncation]\n\
                             {instruction}\n\n\
                             [latest tool result retained after history truncation]\n\
                             {tool_result}"
                        ),
                        None => tool_result.to_string(),
                    });
                }
            } else if let Some(text) = last_user.get("content").and_then(serde_json::Value::as_str)
            {
                if !text.trim().is_empty() {
                    salvaged_user_context = Some(text.to_string());
                }
            }
        }

        if salvaged_user_context.is_none() {
            salvaged_user_context = messages
                .iter()
                .rev()
                .filter(|m| m.get("role").and_then(serde_json::Value::as_str) == Some("user"))
                .filter_map(|m| {
                    m.get("content")
                        .and_then(serde_json::Value::as_array)
                        .and_then(|blocks| latest_non_empty_text_block(blocks))
                })
                .next()
                .map(str::to_string);
        }
    }

    // Compute a generous tail size so the user sees *something* from
    // their last message even after collapse. Half the cap minus
    // overhead is a safe upper bound; `build_truncated_text` clamps
    // again at write time.
    let tail_budget = cap_bytes.saturating_sub(TRUNCATION_MARKER_BUDGET * 2) / 2;
    let salvaged = salvaged_user_context.unwrap_or_else(|| {
        "[The latest user turn contained no recoverable text after request truncation.]".to_string()
    });
    let new_text = build_truncated_text(&salvaged, tail_budget);

    let mut new_messages: Vec<serde_json::Value> = system_messages;
    new_messages.push(serde_json::json!({
        "role": "user",
        "content": [
            { "type": "text", "text": new_text }
        ]
    }));

    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "messages".to_string(),
            serde_json::Value::Array(new_messages),
        );
    }

    serialize_request_body(&value).unwrap_or_else(|_| body_bytes.to_vec())
}

fn latest_non_empty_text_block(blocks: &[serde_json::Value]) -> Option<&str> {
    blocks.iter().rev().find_map(|block| {
        if block.get("type").and_then(serde_json::Value::as_str) != Some("text") {
            return None;
        }
        block
            .get("text")
            .and_then(serde_json::Value::as_str)
            .filter(|text| !text.trim().is_empty())
    })
}

fn largest_non_empty_tool_result(blocks: &[serde_json::Value]) -> Option<&str> {
    blocks
        .iter()
        .filter(|block| {
            block.get("type").and_then(serde_json::Value::as_str) == Some("tool_result")
        })
        .filter_map(|block| {
            let content = block.get("content")?;
            if let Some(text) = content.as_str() {
                return (!text.trim().is_empty()).then_some(text);
            }
            content
                .as_array()?
                .iter()
                .filter_map(|inner| {
                    if inner.get("type").and_then(serde_json::Value::as_str) != Some("text") {
                        return None;
                    }
                    inner
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .filter(|text| !text.trim().is_empty())
                })
                .max_by_key(|text| text.len())
        })
        .max_by_key(|text| text.len())
}

/// Truncate the largest text block in the last user message of an
/// already-serialized Anthropic `/v1/messages` body so the resulting
/// JSON fits under `cap_bytes`. Returns the new serialized body.
///
/// This is the inner loop of the [`fit_body_under_cap`] ladder. It
/// still returns `Err` for "structurally impossible" cases (no
/// `messages` array, no user message, cap smaller than the marker
/// budget) so the outer ladder can decide whether to drop history
/// pairs and retry or fall through to a full collapse.
///
/// Behavior:
///
///   * Edits ONE block per call (the largest truncatable payload in
///     the last user message — see [`TruncationLocation`]).
///   * Returns `Err` when there is no user message, no truncatable
///     payload, or the cap is too small to fit even the marker; the
///     outer ladder treats every `Err` as "try harder".
///   * Single re-serialization pass; if the resulting body is still
///     marginally over the cap (e.g. JSON quoting overhead grew),
///     the outer ladder catches it via the `cap + marker_budget`
///     check and falls through to history trimming.
pub(in crate::anthropic::provider) fn truncate_last_user_message_to_cap(
    body_bytes: &[u8],
    cap_bytes: usize,
) -> Result<Vec<u8>, String> {
    let mut value: serde_json::Value = serde_json::from_slice(body_bytes)
        .map_err(|e| format!("re-parse Anthropic body for truncation: {e}"))?;

    let messages = value
        .get_mut("messages")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| "body has no `messages` array".to_string())?;

    let last_user = messages
        .iter_mut()
        .rev()
        .find(|m| m.get("role").and_then(serde_json::Value::as_str) == Some("user"))
        .ok_or_else(|| "no user message available to truncate".to_string())?;

    let content = last_user
        .get_mut("content")
        .ok_or_else(|| "last user message has no `content` field".to_string())?;

    let blocks = content
        .as_array_mut()
        .ok_or_else(|| "last user message `content` is not an array".to_string())?;

    // Find the largest truncatable text payload across all block kinds we
    // know how to shrink. Anthropic accepts at least three shapes inside
    // the last user message that contribute meaningful bytes:
    //   1. `{"type":"text","text":"..."}` (plain user text)
    //   2. `{"type":"tool_result","content":"..."}` (string content)
    //   3. `{"type":"tool_result","content":[{"type":"text","text":"..."}]}`
    // The pre-fix version only handled (1) and bailed with
    // "last user message has no text block to truncate" when the last
    // turn was a tool_result echo from create_task / create_spec / etc,
    // which is exactly when the body crosses the WAF cliff during
    // task-extraction and dev-loop initialization.
    let largest = blocks
        .iter()
        .enumerate()
        .filter_map(|(i, b)| largest_truncatable_in_block(b).map(|(loc, len)| (i, loc, len)))
        .max_by_key(|(_, _, len)| *len);
    let (block_idx, location, original_text_len) =
        largest.ok_or_else(|| "last user message has no truncatable text payload".to_string())?;
    if original_text_len == 0 {
        return Err("largest truncatable payload in last user message is empty".to_string());
    }

    let excess = body_bytes.len().saturating_sub(cap_bytes);
    if excess == 0 {
        return Ok(body_bytes.to_vec());
    }

    let target_text_len = original_text_len
        .saturating_sub(excess)
        .saturating_sub(TRUNCATION_MARKER_BUDGET);
    if target_text_len == 0 {
        return Err(format!(
            "emergency body cap {cap_bytes}B is smaller than non-content overhead; \
             cannot truncate further (original_text_len={original_text_len}, excess={excess})"
        ));
    }

    apply_truncation_at_location(&mut blocks[block_idx], &location, target_text_len)?;

    // Re-serialize with the same WAF-safe Unicode escaping that the
    // initial body went through. If we used the default
    // `serde_json::to_vec` here, every `\u0026`, `\u005b`, etc. that
    // came back through `from_slice -> Value` would be decoded to its
    // literal byte and the WAF-bypass would silently regress the
    // moment the emergency cap fires (which is exactly when we need
    // it most).
    serialize_request_body(&value).map_err(|e| format!("re-serialize truncated body: {e}"))
}

/// Identifies where in a content block a truncatable text payload lives,
/// so the caller can find the LARGEST one across the whole last-user
/// message before deciding what to shrink. Mirrors the three shapes the
/// truncator now understands.
#[derive(Debug, Clone)]
enum TruncationLocation {
    /// `{"type":"text","text":"..."}`
    TextBlock,
    /// `{"type":"tool_result","content":"<string>"}`
    ToolResultString,
    /// `{"type":"tool_result","content":[..., {"type":"text","text":"..."}, ...]}`
    /// `usize` is the index of the inner text block.
    ToolResultArrayText(usize),
}

fn largest_truncatable_in_block(block: &serde_json::Value) -> Option<(TruncationLocation, usize)> {
    let kind = block.get("type").and_then(serde_json::Value::as_str)?;
    match kind {
        "text" => {
            let text_len = block
                .get("text")
                .and_then(serde_json::Value::as_str)
                .map_or(0, str::len);
            Some((TruncationLocation::TextBlock, text_len))
        }
        "tool_result" => {
            let content = block.get("content")?;
            if let Some(s) = content.as_str() {
                Some((TruncationLocation::ToolResultString, s.len()))
            } else if let Some(arr) = content.as_array() {
                arr.iter()
                    .enumerate()
                    .filter_map(|(i, inner)| {
                        let is_text =
                            inner.get("type").and_then(serde_json::Value::as_str) == Some("text");
                        if !is_text {
                            return None;
                        }
                        let len = inner
                            .get("text")
                            .and_then(serde_json::Value::as_str)
                            .map_or(0, str::len);
                        Some((i, len))
                    })
                    .max_by_key(|(_, len)| *len)
                    .map(|(i, len)| (TruncationLocation::ToolResultArrayText(i), len))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn apply_truncation_at_location(
    block: &mut serde_json::Value,
    location: &TruncationLocation,
    target_text_len: usize,
) -> Result<(), String> {
    match location {
        TruncationLocation::TextBlock => {
            let original = block
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let new_text = build_truncated_text(&original, target_text_len);
            let block_obj = block
                .as_object_mut()
                .ok_or_else(|| "text block is not an object".to_string())?;
            block_obj.insert("text".to_string(), serde_json::Value::String(new_text));
        }
        TruncationLocation::ToolResultString => {
            let original = block
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let new_text = build_truncated_text(&original, target_text_len);
            let block_obj = block
                .as_object_mut()
                .ok_or_else(|| "tool_result block is not an object".to_string())?;
            block_obj.insert("content".to_string(), serde_json::Value::String(new_text));
        }
        TruncationLocation::ToolResultArrayText(inner_idx) => {
            let inner_arr = block
                .get_mut("content")
                .and_then(serde_json::Value::as_array_mut)
                .ok_or_else(|| "tool_result content is not an array".to_string())?;
            let inner = inner_arr
                .get_mut(*inner_idx)
                .ok_or_else(|| "tool_result inner index out of bounds".to_string())?;
            let original = inner
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let new_text = build_truncated_text(&original, target_text_len);
            let inner_obj = inner
                .as_object_mut()
                .ok_or_else(|| "tool_result inner text block is not an object".to_string())?;
            inner_obj.insert("text".to_string(), serde_json::Value::String(new_text));
        }
    }
    Ok(())
}

fn build_truncated_text(original: &str, target_text_len: usize) -> String {
    let mut kept = String::with_capacity(target_text_len);
    let mut written = 0usize;
    for ch in original.chars() {
        let ch_len = ch.len_utf8();
        if written + ch_len > target_text_len {
            break;
        }
        kept.push(ch);
        written += ch_len;
    }
    let original_text_len = original.len();
    let kept_len = kept.len();
    format!(
        "{TRUNCATION_MARKER_PREFIX}original_len={original_text_len},kept={kept_len}>>>\n\n{kept}"
    )
}
