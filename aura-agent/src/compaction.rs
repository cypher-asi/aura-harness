//! Context compaction — tiered message truncation to manage token limits.

use aura_reasoner::Message;

/// Compaction tier configuration.
#[derive(Debug, Clone, Copy)]
pub struct CompactionConfig {
    /// Maximum characters for tool results in older messages.
    pub tool_result_max_chars: usize,
    /// Maximum characters for plain text in older messages.
    pub text_max_chars: usize,
    /// Number of recent messages to preserve uncompacted.
    pub preserve_recent: usize,
}

impl CompactionConfig {
    /// Micro tier: very aggressive truncation for near-limit contexts.
    pub const fn micro() -> Self {
        Self {
            tool_result_max_chars: 200,
            text_max_chars: 400,
            preserve_recent: 2,
        }
    }

    /// Aggressive tier: significant truncation for high-utilization contexts.
    pub const fn aggressive() -> Self {
        Self {
            tool_result_max_chars: 500,
            text_max_chars: 800,
            preserve_recent: 4,
        }
    }

    /// History tier: moderate truncation preserving more context.
    pub const fn history() -> Self {
        Self {
            tool_result_max_chars: 1500,
            text_max_chars: 2000,
            preserve_recent: 6,
        }
    }
}

/// Truncate a string to the given max chars, preserving head and tail.
pub fn truncate_content(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }

    let head = max_chars / 3;
    let tail = max_chars / 3;
    let head_part: String = content.chars().take(head).collect();
    let tail_part: String = content
        .chars()
        .rev()
        .take(tail)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    format!(
        "{head_part}\n\n[...content truncated ({} chars omitted)...]\n\n{tail_part}",
        content.len() - head - tail
    )
}

/// Estimate total character count of messages.
pub fn estimate_message_chars(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|m| {
            m.content
                .iter()
                .map(|block| match block {
                    aura_reasoner::ContentBlock::Text { text } => text.len(),
                    aura_reasoner::ContentBlock::Thinking { thinking, .. } => thinking.len(),
                    aura_reasoner::ContentBlock::ToolUse { input, .. } => {
                        serde_json::to_string(input).map_or(0, |s| s.len())
                    }
                    aura_reasoner::ContentBlock::ToolResult { content, .. } => match content {
                        aura_reasoner::ToolResultContent::Text(t) => t.len(),
                        aura_reasoner::ToolResultContent::Json(v) => {
                            serde_json::to_string(v).map_or(0, |s| s.len())
                        }
                    },
                    aura_reasoner::ContentBlock::Image { source } => source.data.len(),
                })
                .sum::<usize>()
        })
        .sum()
}

/// Select the compaction tier based on context utilization percentage.
pub fn select_tier(utilization: f64) -> Option<CompactionConfig> {
    use crate::constants::{
        COMPACTION_TIER_AGGRESSIVE, COMPACTION_TIER_HISTORY, COMPACTION_TIER_MICRO,
    };
    if utilization >= COMPACTION_TIER_HISTORY {
        Some(CompactionConfig::history())
    } else if utilization >= COMPACTION_TIER_AGGRESSIVE {
        Some(CompactionConfig::aggressive())
    } else if utilization >= COMPACTION_TIER_MICRO {
        Some(CompactionConfig::micro())
    } else {
        None
    }
}

/// Compact older messages using the given tier configuration.
///
/// Preserves the first message (cache anchor) and the most recent
/// `config.preserve_recent` messages. Middle messages have their
/// tool results and text content truncated.
pub fn compact_older_messages(messages: &mut [Message], config: &CompactionConfig) {
    if messages.len() <= config.preserve_recent + 1 {
        return;
    }

    let compact_end = messages.len().saturating_sub(config.preserve_recent);

    for msg in &mut messages[1..compact_end] {
        for block in &mut msg.content {
            match block {
                aura_reasoner::ContentBlock::ToolResult { content, .. } => {
                    let text = match content {
                        aura_reasoner::ToolResultContent::Text(t) => t.clone(),
                        aura_reasoner::ToolResultContent::Json(v) => {
                            serde_json::to_string(v).unwrap_or_default()
                        }
                    };
                    if text.len() > config.tool_result_max_chars {
                        *content = aura_reasoner::ToolResultContent::Text(truncate_content(
                            &text,
                            config.tool_result_max_chars,
                        ));
                    }
                }
                aura_reasoner::ContentBlock::Text { text } => {
                    if text.len() > config.text_max_chars {
                        *text = truncate_content(text, config.text_max_chars);
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_below_threshold() {
        let content = "short";
        assert_eq!(truncate_content(content, 100), "short");
    }

    #[test]
    fn test_truncate_preserves_head_and_tail() {
        let content = "a".repeat(300);
        let result = truncate_content(&content, 200);
        assert!(result.contains("content truncated"));
        assert!(result.len() < 300);
    }

    #[test]
    fn test_compact_older_preserves_recent() {
        let mut messages = vec![
            Message::user("first"),
            Message::user("second"),
            Message::user("third"),
            Message::user("fourth"),
        ];
        let config = CompactionConfig {
            tool_result_max_chars: 10,
            text_max_chars: 10,
            preserve_recent: 2,
        };
        compact_older_messages(&mut messages, &config);
        assert_eq!(messages.len(), 4);
    }

    #[test]
    fn test_select_tier_85pct() {
        let tier = select_tier(0.85);
        assert!(tier.is_some());
        let config = tier.unwrap();
        assert_eq!(config.preserve_recent, 6);
    }

    #[test]
    fn test_select_tier_below_threshold() {
        let tier = select_tier(0.10);
        assert!(tier.is_none());
    }
}
