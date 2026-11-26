//! Model-assisted summary escalation: range selection, inputs, and application.

use aura_config::CHARS_PER_TOKEN;
use aura_model::{ContentBlock, Message, Role};

use super::blocks::truncate_content;
use super::config::{CompactionAction, CompactionConfig, CompactionPolicy, CompactionReport};
use super::pressure::request_body_cap;
use super::size::estimate_message_chars;

/// Upper bound on the synthetic summary's target footprint (in chars).
///
/// Used as a sanity ceiling so a request-body-cap-less Chat run can't
/// ask for an arbitrarily large summary target. 96 KiB ≈ 24 K tokens,
/// which is well under any sensible context budget.
const SUMMARY_TARGET_CEILING: usize = 96 * 1024;

/// Input for model-assisted history summary escalation.
#[derive(Debug, Clone)]
pub struct SummaryInput {
    /// Start index of the compactable middle history in the live message vector.
    pub compact_start: usize,
    /// Exclusive end index of the compactable middle history in the live message vector.
    pub compact_end: usize,
    /// Middle history that should be summarized by the caller's model provider.
    pub compactable_messages: Vec<Message>,
    /// Recent tail that must remain available verbatim after summary application.
    pub recent_tail: Vec<Message>,
    /// Estimated characters in the live transcript before summary replacement.
    pub original_chars: usize,
    /// Estimated characters after local compaction/redaction.
    pub local_chars: usize,
    /// Target total transcript size after the synthetic summary is applied.
    pub target_total_chars: usize,
    /// Suggested upper bound for the generated summary text.
    pub max_summary_chars: usize,
}

/// Output of a summary operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SummaryOutput {
    /// Replacement JSON input for a tool-use block.
    Input(serde_json::Value),
    /// Replacement text for a cached tool result.
    Text(String),
    /// Replacement text for compacted middle history.
    Messages {
        /// Model-generated summary for compacted middle history.
        text: String,
        /// Start index of the compacted history range.
        compact_start: usize,
        /// Exclusive end index of the compacted history range.
        compact_end: usize,
    },
}

fn summary_target_total_chars(policy: CompactionPolicy, before_chars: usize) -> usize {
    let mut target = before_chars.saturating_mul(7) / 10;

    if let Some(max_ctx) = policy.max_context_tokens {
        let available_tokens = max_ctx.saturating_sub(policy.reserved_output_tokens);
        let available_chars = usize::try_from(available_tokens)
            .unwrap_or(usize::MAX / CHARS_PER_TOKEN)
            .saturating_mul(CHARS_PER_TOKEN);
        target = target.min(available_chars.saturating_mul(8) / 10);
    }

    if let Some(cap) = request_body_cap(policy) {
        target = target.min(cap.saturating_mul(8) / 10);
    }

    target.clamp(1_024, SUMMARY_TARGET_CEILING)
}

fn message_tool_use_ids(message: &Message) -> Vec<&str> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect()
}

fn message_tool_result_ids(message: &Message) -> Vec<&str> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect()
}

fn result_is_for_previous_tool_use(messages: &[Message], idx: usize) -> bool {
    if idx == 0 {
        return false;
    }
    let result_ids = message_tool_result_ids(&messages[idx]);
    if result_ids.is_empty() {
        return false;
    }
    let previous_use_ids = message_tool_use_ids(&messages[idx - 1]);
    result_ids
        .iter()
        .any(|result_id| previous_use_ids.iter().any(|use_id| use_id == result_id))
}

fn boundary_splits_tool_pair(messages: &[Message], end: usize) -> bool {
    end < messages.len() && result_is_for_previous_tool_use(messages, end)
}

fn select_summary_range(messages: &[Message]) -> Option<(usize, usize)> {
    if messages.len() <= CompactionConfig::emergency().preserve_recent + 2 {
        return None;
    }

    let mut start = 1;
    let mut end = messages
        .len()
        .saturating_sub(CompactionConfig::emergency().preserve_recent);

    while start < end && result_is_for_previous_tool_use(messages, start) {
        start += 1;
    }
    while start < end && boundary_splits_tool_pair(messages, end) {
        end = end.saturating_sub(1);
    }

    (end.saturating_sub(start) >= 4).then_some((start, end))
}

pub(super) fn build_summary_input(
    messages: &[Message],
    policy: CompactionPolicy,
    before_chars: usize,
    local_chars: usize,
) -> Option<SummaryInput> {
    let target_total_chars = summary_target_total_chars(policy, before_chars);
    if local_chars <= target_total_chars {
        return None;
    }

    let (compact_start, compact_end) = select_summary_range(messages)?;
    let compactable_messages = messages[compact_start..compact_end].to_vec();
    let compactable_chars = estimate_message_chars(&compactable_messages);
    if compactable_chars == 0 {
        return None;
    }

    let recent_tail = messages[compact_end..].to_vec();
    let max_summary_chars = compactable_chars
        .saturating_div(4)
        .min(target_total_chars.saturating_div(4))
        .clamp(512, 12_000);

    Some(SummaryInput {
        compact_start,
        compact_end,
        compactable_messages,
        recent_tail,
        original_chars: before_chars,
        local_chars,
        target_total_chars,
        max_summary_chars,
    })
}

/// Summary input for compacting `messages` on request rather than under
/// pressure: everything between the first message and the recent tail,
/// whatever the transcript's size. `None` when that middle is too short
/// to be worth a summary.
#[must_use]
pub fn forced_summary_input(messages: &[Message]) -> Option<SummaryInput> {
    let (compact_start, compact_end) = select_summary_range(messages)?;
    let compactable_messages = messages[compact_start..compact_end].to_vec();
    let compactable_chars = estimate_message_chars(&compactable_messages);
    if compactable_chars == 0 {
        return None;
    }
    let before_chars = estimate_message_chars(messages);
    let max_summary_chars = compactable_chars.saturating_div(4).clamp(512, 12_000);
    let target_total_chars = before_chars
        .saturating_sub(compactable_chars)
        .saturating_add(max_summary_chars)
        .clamp(1_024, SUMMARY_TARGET_CEILING);
    Some(SummaryInput {
        compact_start,
        compact_end,
        compactable_messages,
        recent_tail: messages[compact_end..].to_vec(),
        original_chars: before_chars,
        local_chars: before_chars,
        target_total_chars,
        max_summary_chars,
    })
}

/// Rewrite compactable middle history to a single synthetic summary message.
pub fn apply_message_summary(
    messages: &mut Vec<Message>,
    summary: SummaryOutput,
) -> CompactionReport {
    let before_chars = estimate_message_chars(messages);
    let SummaryOutput::Messages {
        text,
        compact_start,
        compact_end,
    } = summary
    else {
        return unchanged_report(before_chars);
    };

    if !is_applicable_summary(messages, &text, compact_start, compact_end) {
        return unchanged_report(before_chars);
    }

    let removed_chars = estimate_message_chars(&messages[compact_start..compact_end]);
    let synthetic = synthetic_summary_message(text, removed_chars);
    messages.splice(compact_start..compact_end, [synthetic]);

    let mut after_chars = estimate_message_chars(messages);
    if after_chars >= before_chars && messages.len() > compact_start {
        shrink_to_fallback_summary(messages, compact_start, removed_chars);
        after_chars = estimate_message_chars(messages);
    }

    CompactionReport {
        before_chars,
        after_chars,
        action: CompactionAction::Applied(CompactionConfig::emergency()),
    }
}

const fn unchanged_report(before_chars: usize) -> CompactionReport {
    CompactionReport {
        before_chars,
        after_chars: before_chars,
        action: CompactionAction::None,
    }
}

/// Whether `text` may replace `messages[compact_start..compact_end]`: the
/// summary is non-empty, the range is in bounds, keeps the first message,
/// and does not split a tool_use/tool_result pair at either edge.
fn is_applicable_summary(
    messages: &[Message],
    text: &str,
    compact_start: usize,
    compact_end: usize,
) -> bool {
    !(text.trim().is_empty()
        || compact_start == 0
        || compact_start >= compact_end
        || compact_end > messages.len()
        || result_is_for_previous_tool_use(messages, compact_start)
        || boundary_splits_tool_pair(messages, compact_end))
}

fn synthetic_summary_message(text: String, removed_chars: usize) -> Message {
    let max_summary_chars = removed_chars.saturating_sub(1).clamp(256, 12_000);
    let summary_text = if text.len() > max_summary_chars {
        truncate_content(&text, max_summary_chars, None, None)
    } else {
        text
    };
    Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: format!(
                "[summary of compacted prior conversation]\n\n{}",
                summary_text.trim()
            ),
        }],
    }
}

/// Replace the synthetic summary at `index` with a harder-truncated copy
/// when the summary did not make the transcript smaller.
fn shrink_to_fallback_summary(messages: &mut [Message], index: usize, removed_chars: usize) {
    let fallback_cap = removed_chars.saturating_div(4).clamp(128, 4_000);
    let fallback = truncate_content(
        messages[index].text_content().as_str(),
        fallback_cap,
        None,
        None,
    );
    messages[index] = Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text { text: fallback }],
    };
}
