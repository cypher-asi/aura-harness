//! Wire-level guards for Anthropic's `tool_use` / `tool_result` pairing
//! rules, applied to the converted messages just before they are sent.

use std::collections::{HashMap, HashSet};

use crate::anthropic::api_types::{ApiContent, ApiMessage};

/// Position of a content block: (message index, block index).
type BlockPosition = (usize, usize);

/// Ids of the `tool_use` blocks in `message`, in order.
fn tool_use_ids(message: &ApiMessage) -> Vec<String> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ApiContent::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

/// Final wire-level guard for the Anthropic positional pairing rule: every
/// `tool_result` block in message `i` must reference a `tool_use` block in
/// message `i - 1`. Violating blocks are dropped (and logged) instead of
/// shipped, because the API rejects the entire request with 400
/// "unexpected `tool_use_id` found in `tool_result` blocks" otherwise.
///
/// Upstream sanitization (`aura-agent`'s `validate_and_repair`) repairs the
/// transcript properly — including injecting synthetic results for any
/// `tool_use` left unpaired — so this guard firing means an upstream path
/// produced an invalid layout; the error log is the signal to fix it there.
pub(in crate::anthropic) fn drop_misplaced_tool_results(api_messages: &mut Vec<ApiMessage>) {
    let mut i = 0;
    while i < api_messages.len() {
        let previous_tool_use_ids: HashSet<String> = if i == 0 {
            HashSet::new()
        } else {
            tool_use_ids(&api_messages[i - 1]).into_iter().collect()
        };

        api_messages[i].content.retain(|block| match block {
            ApiContent::ToolResult { tool_use_id, .. } => {
                let keep = previous_tool_use_ids.contains(tool_use_id);
                if !keep {
                    tracing::error!(
                        message_index = i,
                        tool_use_id = %tool_use_id,
                        "convert_messages_to_api: dropped tool_result without matching tool_use \
                         in the previous message; upstream history construction is invalid"
                    );
                }
                keep
            }
            _ => true,
        });

        if api_messages[i].content.is_empty() {
            // Removing the emptied message changes who the next message's
            // predecessor is, so do not advance: re-evaluate index `i`
            // against its new neighbor on the next pass.
            api_messages.remove(i);
        } else {
            i += 1;
        }
    }
}

/// Companion to [`drop_misplaced_tool_results`]: ensure every assistant
/// `tool_use` has a matching `tool_result` at the front of the immediately
/// following user message, injecting synthetic error results when missing.
/// Without this, dropping a misplaced result would just trade one 400
/// ("unexpected tool_use_id in tool_result") for another ("tool_use ids were
/// found without tool_result blocks immediately after").
pub(in crate::anthropic) fn inject_missing_tool_results(api_messages: &mut Vec<ApiMessage>) {
    let mut i = 0;
    while i < api_messages.len() {
        let tool_use_ids = if api_messages[i].role == "assistant" {
            tool_use_ids(&api_messages[i])
        } else {
            Vec::new()
        };

        if tool_use_ids.is_empty() {
            i += 1;
            continue;
        }

        let existing_result_ids = user_tool_result_ids(api_messages.get(i + 1));
        let synthetic = synthetic_tool_results(i, tool_use_ids, &existing_result_ids);
        if !synthetic.is_empty() {
            insert_after(api_messages, i, synthetic);
        }

        i += 1;
    }
}

/// Ids the `tool_result` blocks of `message` answer, when it is a user message.
fn user_tool_result_ids(message: Option<&ApiMessage>) -> HashSet<String> {
    message
        .filter(|m| m.role == "user")
        .map(|m| {
            m.content
                .iter()
                .filter_map(|block| match block {
                    ApiContent::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Error results for the ids in `tool_use_ids` (of message `message_index`)
/// that `existing_result_ids` does not answer.
fn synthetic_tool_results(
    message_index: usize,
    tool_use_ids: Vec<String>,
    existing_result_ids: &HashSet<String>,
) -> Vec<ApiContent> {
    tool_use_ids
        .into_iter()
        .filter(|id| !existing_result_ids.contains(id))
        .map(|id| {
            tracing::error!(
                message_index = message_index,
                tool_use_id = %id,
                "convert_messages_to_api: injected synthetic tool_result for unpaired tool_use"
            );
            ApiContent::ToolResult {
                tool_use_id: id,
                content: "[Tool result was lost during context compaction]".to_string(),
                is_error: Some(true),
                cache_control: None,
            }
        })
        .collect()
}

/// Put `synthetic` at the front of the user message after message `i`, or
/// in a new user message there when the next message is not one.
fn insert_after(api_messages: &mut Vec<ApiMessage>, i: usize, synthetic: Vec<ApiContent>) {
    if i + 1 < api_messages.len() && api_messages[i + 1].role == "user" {
        for (offset, block) in synthetic.into_iter().enumerate() {
            api_messages[i + 1].content.insert(offset, block);
        }
    } else {
        api_messages.insert(
            i + 1,
            ApiMessage {
                role: "user".to_string(),
                content: synthetic,
            },
        );
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
pub(in crate::anthropic) fn dedupe_tool_results(api_messages: &mut Vec<ApiMessage>) {
    let positions_by_id = tool_result_positions(api_messages);

    let mut to_remove: HashSet<BlockPosition> = HashSet::new();

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

        if !keep_last_body_at_first(api_messages, positions) {
            continue;
        }

        for pos in positions.iter().skip(1) {
            to_remove.insert(*pos);
        }
    }

    if to_remove.is_empty() {
        return;
    }

    remove_blocks(api_messages, &to_remove);
}

/// Positions of the `tool_result` blocks of each non-empty `tool_use_id`,
/// in message order.
fn tool_result_positions(api_messages: &[ApiMessage]) -> HashMap<String, Vec<BlockPosition>> {
    let mut positions_by_id: HashMap<String, Vec<BlockPosition>> = HashMap::new();

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
    positions_by_id
}

/// Copy the body of the last `tool_result` in `positions` onto the first.
/// `false` (and nothing changed) when the last position holds no
/// `tool_result`.
fn keep_last_body_at_first(api_messages: &mut [ApiMessage], positions: &[BlockPosition]) -> bool {
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
            _ => return false,
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
    true
}

/// Remove the blocks at `to_remove`, then every message left empty.
fn remove_blocks(api_messages: &mut Vec<ApiMessage>, to_remove: &HashSet<BlockPosition>) {
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
