//! Message sanitization — repairs message history for API validity.
//!
//! Runs 6 passes:
//! 1. Remove empty messages
//! 2. Merge consecutive same-role messages
//! 3. Fix orphan tool results (`tool_result` without a matching `tool_use`
//!    in the immediately previous message — Anthropic positional rule)
//! 4. Fix unpaired tool uses (`tool_use` without matching `tool_result`)
//! 5. Ensure conversation starts with a user message
//! 6. Assert positional `tool_use/tool_result` constraint (debug guard)

use crate::dup_audit;
use aura_model::{ContentBlock, Message, Role, ToolResultContent};
use std::collections::HashSet;
use tracing::warn;

/// Run all sanitization passes on the message history.
pub fn validate_and_repair(messages: &mut Vec<Message>) {
    dup_audit::audit_tool_result_duplicates(messages, "sanitize.entry");
    remove_empty_messages(messages);
    merge_consecutive_same_role(messages);
    dup_audit::audit_tool_result_duplicates(messages, "sanitize.after_merge");
    fix_orphan_tool_results(messages);
    // Dropping orphan-only messages can leave two same-role neighbors
    // adjacent again, so re-merge before the pairing passes below.
    merge_consecutive_same_role(messages);
    dup_audit::audit_tool_result_duplicates(messages, "sanitize.after_orphan");
    fix_unpaired_tool_uses(messages);
    dup_audit::audit_tool_result_duplicates(messages, "sanitize.after_unpaired");
    ensure_starts_with_user(messages);
    debug_assert_tool_pairing(messages);
}

/// Pass 1: Remove messages with no content blocks or only empty text.
fn remove_empty_messages(messages: &mut Vec<Message>) {
    messages.retain(|msg| {
        !msg.content.is_empty()
            && msg.content.iter().any(|block| match block {
                ContentBlock::Text { text } => !text.trim().is_empty(),
                _ => true,
            })
    });
}

/// Pass 2: Merge consecutive messages with the same role.
fn merge_consecutive_same_role(messages: &mut Vec<Message>) {
    if messages.len() < 2 {
        return;
    }

    let mut i = 0;
    while i + 1 < messages.len() {
        if messages[i].role == messages[i + 1].role {
            let next_content = messages[i + 1].content.clone();
            messages[i].content.extend(next_content);
            messages.remove(i + 1);
        } else {
            i += 1;
        }
    }
}

/// Pass 3: Remove orphan tool results that don't have a matching `tool_use`
/// in the **immediately previous** message.
///
/// The Anthropic Messages API enforces this positionally: "Each `tool_result`
/// block must have a corresponding `tool_use` block in the previous message."
/// A global membership check is not enough — a `tool_result` whose `tool_use`
/// survives *elsewhere* in the transcript (e.g. after compaction splicing or
/// same-role merges rearranged history) still 400s the whole request, worst
/// case as `messages.0.content.0` when the result ends up at the very front.
/// Any `tool_use` left without a result by this pass gets a synthetic result
/// injected by pass 4, so the transcript converges to a valid state.
fn fix_orphan_tool_results(messages: &mut Vec<Message>) {
    for i in 0..messages.len() {
        let previous_tool_use_ids: HashSet<String> = if i == 0 {
            HashSet::new()
        } else {
            messages[i - 1]
                .content
                .iter()
                .filter_map(|block| {
                    if let ContentBlock::ToolUse { id, .. } = block {
                        Some(id.clone())
                    } else {
                        None
                    }
                })
                .collect()
        };

        messages[i].content.retain(|block| {
            if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                let keep = previous_tool_use_ids.contains(tool_use_id);
                if !keep {
                    warn!(
                        message_index = i,
                        tool_use_id = tool_use_id.as_str(),
                        "Sanitization: dropped tool_result without matching tool_use in the previous message"
                    );
                }
                keep
            } else {
                true
            }
        });
    }

    messages.retain(|msg| !msg.content.is_empty());
}

/// Pass 4: Ensure every assistant `tool_use` has a matching `tool_result`
/// in the **immediately following** user message (Anthropic positional rule).
///
/// Injects synthetic error results for any missing pairings.
fn fix_unpaired_tool_uses(messages: &mut Vec<Message>) {
    let mut i = 0;
    while i < messages.len() {
        if messages[i].role != Role::Assistant {
            i += 1;
            continue;
        }

        let tool_use_ids: Vec<String> = tool_use_ids(&messages[i])
            .into_iter()
            .map(str::to_string)
            .collect();

        if tool_use_ids.is_empty() {
            i += 1;
            continue;
        }

        let existing_result_ids: HashSet<String> = next_user_result_ids(messages, i)
            .into_iter()
            .map(str::to_string)
            .collect();

        let missing: Vec<String> = tool_use_ids
            .into_iter()
            .filter(|id| !existing_result_ids.contains(id))
            .collect();

        if !missing.is_empty() {
            let synthetic: Vec<ContentBlock> = missing
                .into_iter()
                .map(|id| {
                    ContentBlock::tool_result(
                        &id,
                        ToolResultContent::text("[Tool result was lost during context compaction]"),
                        true,
                    )
                })
                .collect();

            if i + 1 < messages.len() && messages[i + 1].role == Role::User {
                for (offset, block) in synthetic.into_iter().enumerate() {
                    messages[i + 1].content.insert(offset, block);
                }
            } else {
                messages.insert(i + 1, Message::new(Role::User, synthetic));
            }
        }

        i += 1;
    }
}

/// Pass 5: Ensure the conversation starts with a user message.
fn ensure_starts_with_user(messages: &mut Vec<Message>) {
    if messages.is_empty() || messages[0].role != Role::User {
        messages.insert(0, Message::user("[System: conversation context]"));
    }
}

/// Pass 6 (guard): Verify both directions of the positional pairing rule —
/// every assistant `tool_use` is immediately followed by a user message
/// containing the matching `tool_result`, and every `tool_result` references
/// a `tool_use` in the immediately previous message. Logs a warning on any
/// violation so it surfaces in traces rather than silently hitting the
/// Anthropic 400 error.
fn debug_assert_tool_pairing(messages: &[Message]) {
    for (i, msg) in messages.iter().enumerate() {
        for block in &msg.content {
            let ContentBlock::ToolResult { tool_use_id, .. } = block else {
                continue;
            };
            let paired_with_previous = i > 0
                && messages[i - 1]
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolUse { id, .. } if id == tool_use_id));
            if !paired_with_previous {
                warn!(
                    message_index = i,
                    tool_use_id = tool_use_id.as_str(),
                    "Sanitization guard: tool_result without matching tool_use in the previous message"
                );
            }
        }
    }

    for (i, msg) in messages.iter().enumerate() {
        if msg.role != Role::Assistant {
            continue;
        }

        let tool_use_ids: Vec<&str> = tool_use_ids(msg);

        if tool_use_ids.is_empty() {
            continue;
        }

        let next_result_ids: HashSet<&str> = next_user_result_ids(messages, i);

        for id in &tool_use_ids {
            if !next_result_ids.contains(id) {
                warn!(
                    message_index = i,
                    tool_use_id = id,
                    "Sanitization guard: tool_use without matching tool_result in next message"
                );
            }
        }
    }
}

/// Ids of the `tool_use` blocks in `msg`, in order.
fn tool_use_ids(msg: &Message) -> Vec<&str> {
    msg.content
        .iter()
        .filter_map(|b| {
            if let ContentBlock::ToolUse { id, .. } = b {
                Some(id.as_str())
            } else {
                None
            }
        })
        .collect()
}

/// `tool_use_id`s answered by the message after index `i`, when that message
/// is a user message; empty otherwise.
fn next_user_result_ids(messages: &[Message], i: usize) -> HashSet<&str> {
    messages
        .get(i + 1)
        .filter(|m| m.role == Role::User)
        .map(|m| {
            m.content
                .iter()
                .filter_map(|b| {
                    if let ContentBlock::ToolResult { tool_use_id, .. } = b {
                        Some(tool_use_id.as_str())
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
