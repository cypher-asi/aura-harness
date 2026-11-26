mod truncate;

use super::waf::serialize_request_body;
pub(super) use truncate::{collapse_messages_to_marker, truncate_last_user_message_to_cap};

/// Marker prepended to the truncated text block so downstream tools,
/// logs, and the LLM itself can spot a Phase-0 truncation. Format:
///   `<<<AURA_HARNESS_EMERGENCY_TRUNCATED:original_len=N,kept=M>>>`
///
/// The marker is stable (no timestamps / random tokens) so a
/// `grep AURA_HARNESS_EMERGENCY_TRUNCATED` over a transcript pinpoints
/// every truncated request.
pub(super) const TRUNCATION_MARKER_PREFIX: &str = "<<<AURA_HARNESS_EMERGENCY_TRUNCATED:";

/// Generous estimate of the maximum length a truncation marker can
/// reach (`<<<AURA_HARNESS_EMERGENCY_TRUNCATED:original_len=…,kept=…>>>`
/// plus a `\n\n` separator). 128 bytes leaves headroom for very large
/// `original_len` / `kept` numbers without ever underflowing the cap.
pub(super) const TRUNCATION_MARKER_BUDGET: usize = 128;

/// Outcome marker for [`fit_body_under_cap`]: tells the caller which
/// path through the trimming ladder fit the body. Used purely for
/// observability — the bytes the caller receives are valid in every
/// case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BodyFitMode {
    /// Body was already at or under the cap — no edits made.
    NoOp,
    /// Last user message's largest text/tool_result block was truncated.
    TruncatedLastUser,
    /// One or more pairs of oldest non-system messages were dropped
    /// before truncating the last user message.
    DroppedOldestPairs,
    /// Replaced base64 image blocks in all but the last message with
    /// text stubs. Images dominate oversized bodies and the model has
    /// already consumed older ones, so this preserves the full text
    /// history where dropping pairs would not.
    StubbedOlderImages,
    /// Last-resort: replaced the entire message history with a
    /// single placeholder user turn carrying the truncation marker.
    Collapsed,
    /// Body could not be parsed as JSON — the original bytes are
    /// returned unchanged. We deliberately do not error because that
    /// would mask the much more informative upstream response.
    Unparseable,
}

impl std::fmt::Display for BodyFitMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            BodyFitMode::NoOp => "noop",
            BodyFitMode::TruncatedLastUser => "truncated_last_user",
            BodyFitMode::DroppedOldestPairs => "dropped_oldest_pairs",
            BodyFitMode::StubbedOlderImages => "stubbed_older_images",
            BodyFitMode::Collapsed => "collapsed_to_marker",
            BodyFitMode::Unparseable => "unparseable",
        };
        f.write_str(s)
    }
}

/// Always-succeeds body-shaper.
///
/// Given a serialised `/v1/messages` body and a hard cap in bytes,
/// return a new body that is at or below the cap (modulo the small,
/// fixed truncation-marker budget). Trimming follows a ladder so we
/// drop the *least* informative bytes first:
///
///   1. **No-op.** Body already fits.
///   2. **Truncate last user.** Shrink the largest text /
///      `tool_result` payload in the last user message. This is the
///      common case — one giant pasted blob or one huge tool result.
///   3. **Stub older images.** Replace base64 image blocks in all but
///      the last message with text stubs. Image payloads dominate
///      oversized bodies; stubbing them preserves the entire text
///      history where dropping pairs would discard it.
///   4. **Drop oldest message pairs.** When the rest of the
///      transcript is the bulk of the body (long agent loops), drop
///      the oldest user/assistant pair (system messages are
///      preserved) and re-attempt step 2. Each drop is followed by an
///      orphan-`tool_result` repair pass so the positional
///      `tool_use`/`tool_result` pairing rule still holds.
///   5. **Collapse.** Last-ditch: replace `messages` with a single
///      synthetic user turn `[history elided due to body cap]\n<original last user text>`.
///
/// The function never returns `Err`; if the body can't even be
/// parsed back to JSON we return the original bytes with a
/// `BodyFitMode::Unparseable` marker so the caller can log it.
pub(super) fn fit_body_under_cap(
    body_bytes: &[u8],
    cap_bytes: usize,
) -> (Vec<u8>, usize, BodyFitMode) {
    if cap_bytes == 0 || body_bytes.len() <= cap_bytes {
        return (body_bytes.to_vec(), 0, BodyFitMode::NoOp);
    }

    let parsed: serde_json::Value = match serde_json::from_slice(body_bytes) {
        Ok(v) => v,
        Err(_) => return (body_bytes.to_vec(), 0, BodyFitMode::Unparseable),
    };

    // Try the cheap path first.
    if let Ok(truncated) = truncate_last_user_message_to_cap(body_bytes, cap_bytes) {
        if truncated.len() <= cap_bytes + TRUNCATION_MARKER_BUDGET {
            return (truncated, 0, BodyFitMode::TruncatedLastUser);
        }
    }

    let mut value = parsed;

    // Image-stub path: base64 images are usually the bulk of an
    // oversized body. Stubbing the ones the model already consumed
    // (everything outside the last message) keeps the full text
    // history intact, which dropping pairs below would not.
    if stub_images_in_older_messages(&mut value) > 0 {
        if let Ok(candidate) = serialize_request_body(&value) {
            if candidate.len() <= cap_bytes {
                return (candidate, 0, BodyFitMode::StubbedOlderImages);
            }
            if let Ok(truncated) = truncate_last_user_message_to_cap(&candidate, cap_bytes) {
                if truncated.len() <= cap_bytes + TRUNCATION_MARKER_BUDGET {
                    return (truncated, 0, BodyFitMode::StubbedOlderImages);
                }
            }
        }
    }

    // History-trimming path: drop oldest non-system pair, re-truncate, repeat.
    let mut dropped = 0usize;
    for _ in 0..MAX_HISTORY_DROP_ITERATIONS {
        if drop_oldest_non_system_message_pair(&mut value).is_err() {
            break;
        }
        dropped += 2;
        strip_orphan_tool_results(&mut value);

        let Ok(candidate) = serialize_request_body(&value) else {
            break;
        };
        if candidate.len() <= cap_bytes {
            return (candidate, dropped, BodyFitMode::DroppedOldestPairs);
        }
        if let Ok(truncated) = truncate_last_user_message_to_cap(&candidate, cap_bytes) {
            if truncated.len() <= cap_bytes + TRUNCATION_MARKER_BUDGET {
                return (truncated, dropped, BodyFitMode::DroppedOldestPairs);
            }
        }
    }

    // Last resort: collapse the entire message history.
    let collapsed_bytes = collapse_messages_to_marker(body_bytes, cap_bytes);
    (collapsed_bytes, dropped, BodyFitMode::Collapsed)
}

/// Safety bound on the history-trim loop so a pathological body
/// (e.g. tens of thousands of messages, each individually huge) can't
/// burn cycles indefinitely. 64 iterations × 2 messages = 128
/// messages dropped, which is more than any real chat history.
const MAX_HISTORY_DROP_ITERATIONS: usize = 64;

/// Replace base64 image blocks in every message except the last with a
/// text stub, returning how many images were stubbed. The last message
/// is exempt so images attached on the current turn (user uploads,
/// fresh tool screenshots) still reach the model.
fn stub_images_in_older_messages(value: &mut serde_json::Value) -> usize {
    let Some(messages) = value
        .get_mut("messages")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return 0;
    };
    let len = messages.len();
    if len < 2 {
        return 0;
    }

    let mut stubbed = 0usize;
    for msg in &mut messages[..len - 1] {
        let Some(blocks) = msg
            .get_mut("content")
            .and_then(serde_json::Value::as_array_mut)
        else {
            continue;
        };
        for block in blocks.iter_mut() {
            if block.get("type").and_then(serde_json::Value::as_str) != Some("image") {
                continue;
            }
            let kb = block
                .get("source")
                .and_then(|s| s.get("data"))
                .and_then(serde_json::Value::as_str)
                .map_or(0, |d| d.len() / 1024);
            *block = serde_json::json!({
                "type": "text",
                "text": format!("[image removed to fit the request size limit: ~{kb} KB base64]")
            });
            stubbed += 1;
        }
    }
    stubbed
}

/// Strip `tool_result` blocks that violate Anthropic's positional rule:
/// each `tool_result` in message `i` must reference a `tool_use` in
/// message `i - 1`. Dropping the oldest message pair can remove an
/// assistant `tool_use` while the next user message (now at the front)
/// still carries its `tool_result`, which the API rejects with 400
/// "messages.0.content.0: unexpected `tool_use_id` found in
/// `tool_result` blocks". A message left with no content blocks gets a
/// marker text block instead of being removed, so the first message
/// keeps the `user` role and the alternation cadence is preserved.
fn strip_orphan_tool_results(value: &mut serde_json::Value) {
    let Some(messages) = value
        .get_mut("messages")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };

    let mut previous_tool_use_ids: Vec<String> = Vec::new();
    for msg in messages.iter_mut() {
        let current_tool_use_ids: Vec<String> = msg
            .get("content")
            .and_then(serde_json::Value::as_array)
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| {
                        b.get("type").and_then(serde_json::Value::as_str) == Some("tool_use")
                    })
                    .filter_map(|b| {
                        b.get("id")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default();

        if let Some(blocks) = msg
            .get_mut("content")
            .and_then(serde_json::Value::as_array_mut)
        {
            blocks.retain(|b| {
                if b.get("type").and_then(serde_json::Value::as_str) == Some("tool_result") {
                    b.get("tool_use_id")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|id| previous_tool_use_ids.iter().any(|p| p == id))
                } else {
                    true
                }
            });
            if blocks.is_empty() {
                blocks.push(serde_json::json!({
                    "type": "text",
                    "text": "[earlier tool results were removed to fit the request size limit]"
                }));
            }
        }

        previous_tool_use_ids = current_tool_use_ids;
    }
}

/// Drop the oldest pair of non-system messages from a parsed body.
/// Returns `Err` when fewer than two non-system messages remain (the
/// caller must fall back to collapse mode).
///
/// Dropping in pairs preserves the user/assistant cadence. It does NOT
/// by itself preserve `tool_use`/`tool_result` pairing — removing an
/// assistant message that issued tool calls strands the results in the
/// following user message — so callers must run
/// [`strip_orphan_tool_results`] after each drop. System messages are
/// immutable here.
fn drop_oldest_non_system_message_pair(value: &mut serde_json::Value) -> Result<(), ()> {
    let messages = value
        .get_mut("messages")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or(())?;

    let oldest_idx = messages
        .iter()
        .position(|m| m.get("role").and_then(serde_json::Value::as_str) != Some("system"))
        .ok_or(())?;

    // Need at least one more non-system message after the one we're about
    // to drop, otherwise we'd leave a dangling pair half-removed.
    let second_idx = messages
        .iter()
        .enumerate()
        .skip(oldest_idx + 1)
        .find(|(_, m)| m.get("role").and_then(serde_json::Value::as_str) != Some("system"))
        .map(|(i, _)| i)
        .ok_or(())?;

    messages.remove(second_idx);
    messages.remove(oldest_idx);

    // Refuse to collapse the conversation to zero non-system messages
    // here — the caller decides whether to fall through to
    // `collapse_messages_to_marker`.
    let any_non_system_remaining = messages
        .iter()
        .any(|m| m.get("role").and_then(serde_json::Value::as_str) != Some("system"));
    if !any_non_system_remaining {
        return Err(());
    }
    Ok(())
}
