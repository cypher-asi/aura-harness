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
    /// Micro tier: very aggressive truncation for near-limit contexts (≥85%).
    pub const fn micro() -> Self {
        Self {
            tool_result_max_chars: 200,
            text_max_chars: 400,
            preserve_recent: 2,
        }
    }

    /// Aggressive tier: significant truncation for high-utilization contexts (≥70%).
    pub const fn aggressive() -> Self {
        Self {
            tool_result_max_chars: 500,
            text_max_chars: 800,
            preserve_recent: 4,
        }
    }

    /// Moderate tier: balanced truncation at medium-high utilization (≥60%).
    pub const fn moderate() -> Self {
        Self {
            tool_result_max_chars: 1000,
            text_max_chars: 1500,
            preserve_recent: 6,
        }
    }

    /// Light tier: gentle truncation for moderate utilization (≥30%).
    pub const fn light() -> Self {
        Self {
            tool_result_max_chars: 3000,
            text_max_chars: 4000,
            preserve_recent: 8,
        }
    }

    /// History tier: minimal truncation preserving most context (≥15%).
    pub const fn history() -> Self {
        Self {
            tool_result_max_chars: 1500,
            text_max_chars: 2000,
            preserve_recent: 6,
        }
    }
}

/// Truncate a string to the given max chars, preserving head and tail.
///
/// `head_chars` and `tail_chars` control how many characters to keep from
/// the beginning and end respectively. Pass `None` to use 1/3 of `max_chars`.
pub fn truncate_content(
    content: &str,
    max_chars: usize,
    head_chars: Option<usize>,
    tail_chars: Option<usize>,
) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }

    let head = head_chars.unwrap_or(max_chars / 3);
    let tail = tail_chars.unwrap_or(max_chars / 3);

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
    format!(
        "{head_part}\n\n[...content truncated ({omitted} chars omitted)...]\n\n{tail_part}",
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
///
/// Higher utilization → more aggressive compaction. Returns `None` below 15%.
pub fn select_tier(utilization: f64) -> Option<CompactionConfig> {
    use crate::constants::{
        COMPACTION_TIER_30, COMPACTION_TIER_60, COMPACTION_TIER_AGGRESSIVE, COMPACTION_TIER_HISTORY,
        COMPACTION_TIER_MICRO,
    };
    if utilization >= COMPACTION_TIER_HISTORY {
        Some(CompactionConfig::micro())
    } else if utilization >= COMPACTION_TIER_AGGRESSIVE {
        Some(CompactionConfig::aggressive())
    } else if utilization >= COMPACTION_TIER_60 {
        Some(CompactionConfig::moderate())
    } else if utilization >= COMPACTION_TIER_30 {
        Some(CompactionConfig::light())
    } else if utilization >= COMPACTION_TIER_MICRO {
        Some(CompactionConfig::history())
    } else {
        None
    }
}

/// Best-effort Rust signature extraction.
///
/// If `content` looks like Rust code, replaces function/method bodies with
/// a placeholder, keeping signatures and structural items visible.
/// Returns `None` if the content doesn't look like Rust or the extraction
/// doesn't reduce size by at least 30%.
pub fn try_signature_compact(content: &str) -> Option<String> {
    const RUST_MARKERS: &[&str] = &["fn ", "pub fn", "struct ", "impl ", "mod "];
    let has_rust = RUST_MARKERS.iter().any(|m| content.contains(m));
    if !has_rust {
        return None;
    }

    let mut result = String::with_capacity(content.len());
    let mut chars = content.chars().peekable();
    let mut line_buf = String::new();
    let mut brace_depth: i32 = 0;
    let mut in_body = false;
    let mut body_start_depth: i32 = 0;
    let mut wrote_placeholder = false;

    while let Some(ch) = chars.next() {
        if ch == '\n' || chars.peek().is_none() {
            if ch != '\n' {
                line_buf.push(ch);
            }

            let trimmed = line_buf.trim_start();
            let is_sig_line = trimmed.starts_with("pub fn ")
                || trimmed.starts_with("fn ")
                || trimmed.starts_with("pub(crate) fn ")
                || trimmed.starts_with("pub async fn ")
                || trimmed.starts_with("async fn ")
                || trimmed.starts_with("pub unsafe fn ")
                || trimmed.starts_with("unsafe fn ")
                || trimmed.starts_with("pub const fn ")
                || trimmed.starts_with("const fn ");

            if !in_body {
                if is_sig_line && line_buf.contains('{') {
                    result.push_str(&line_buf);
                    result.push('\n');

                    let open_count = line_buf.chars().filter(|&c| c == '{').count() as i32;
                    let close_count = line_buf.chars().filter(|&c| c == '}').count() as i32;
                    brace_depth += open_count - close_count;

                    if brace_depth > 0 {
                        in_body = true;
                        body_start_depth = brace_depth - 1;
                        wrote_placeholder = false;
                    }
                } else {
                    for c in line_buf.chars() {
                        match c {
                            '{' => brace_depth += 1,
                            '}' => brace_depth -= 1,
                            _ => {}
                        }
                    }
                    result.push_str(&line_buf);
                    result.push('\n');
                }
            } else {
                for c in line_buf.chars() {
                    match c {
                        '{' => brace_depth += 1,
                        '}' => brace_depth -= 1,
                        _ => {}
                    }
                }

                if brace_depth <= body_start_depth {
                    if !wrote_placeholder {
                        result.push_str("    // ... body omitted ...\n");
                    }
                    result.push_str(&line_buf);
                    result.push('\n');
                    in_body = false;
                } else if !wrote_placeholder {
                    result.push_str("    // ... body omitted ...\n");
                    wrote_placeholder = true;
                }
            }

            line_buf.clear();
        } else {
            line_buf.push(ch);
        }
    }

    let reduction = 1.0 - (result.len() as f64 / content.len() as f64);
    if reduction >= 0.30 {
        Some(result)
    } else {
        None
    }
}

/// Compact older messages using the given tier configuration.
///
/// Preserves the first message (cache anchor) and the most recent
/// `config.preserve_recent` messages. Middle messages have their
/// tool results and text content truncated.
///
/// For text blocks, attempts Rust signature extraction before falling back to
/// head/tail truncation. For the micro tier, uses asymmetric head=6000/tail=3000.
pub fn compact_older_messages(messages: &mut [Message], config: &CompactionConfig) {
    if messages.len() <= config.preserve_recent + 1 {
        return;
    }

    let compact_end = messages.len().saturating_sub(config.preserve_recent);

    let is_micro = config.preserve_recent == CompactionConfig::micro().preserve_recent
        && config.tool_result_max_chars == CompactionConfig::micro().tool_result_max_chars;

    let (head_chars, tail_chars) = if is_micro {
        (Some(6000_usize), Some(3000_usize))
    } else {
        (None, None)
    };

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
                        let compacted = try_signature_compact(&text).unwrap_or_else(|| {
                            truncate_content(
                                &text,
                                config.tool_result_max_chars,
                                head_chars,
                                tail_chars,
                            )
                        });
                        if compacted.len() <= config.tool_result_max_chars || compacted.len() < text.len() {
                            *content = aura_reasoner::ToolResultContent::Text(compacted);
                        } else {
                            *content = aura_reasoner::ToolResultContent::Text(truncate_content(
                                &text,
                                config.tool_result_max_chars,
                                head_chars,
                                tail_chars,
                            ));
                        }
                    }
                }
                aura_reasoner::ContentBlock::Text { text } => {
                    if text.len() > config.text_max_chars {
                        if let Some(sig) = try_signature_compact(text) {
                            if sig.len() <= config.text_max_chars || sig.len() < text.len() {
                                *text = sig;
                            } else {
                                *text = truncate_content(
                                    text,
                                    config.text_max_chars,
                                    head_chars,
                                    tail_chars,
                                );
                            }
                        } else {
                            *text = truncate_content(
                                text,
                                config.text_max_chars,
                                head_chars,
                                tail_chars,
                            );
                        }
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
        assert_eq!(truncate_content(content, 100, None, None), "short");
    }

    #[test]
    fn test_truncate_preserves_head_and_tail() {
        let content = "a".repeat(300);
        let result = truncate_content(&content, 200, None, None);
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
        assert_eq!(config.preserve_recent, CompactionConfig::micro().preserve_recent);
        assert_eq!(config.tool_result_max_chars, CompactionConfig::micro().tool_result_max_chars);
    }

    #[test]
    fn test_select_tier_below_threshold() {
        let tier = select_tier(0.10);
        assert!(tier.is_none());
    }

    // ---- New tests ----

    #[test]
    fn test_signature_extract_rust() {
        let rust_code = r#"
use std::io;

pub fn compute_sum(a: i32, b: i32) -> i32 {
    let result = a + b;
    println!("sum = {}", result);
    if result > 100 {
        panic!("too big");
    }
    result
}

pub struct Config {
    pub name: String,
    pub value: u64,
}

impl Config {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            value: 0,
        }
    }

    pub fn set_value(&mut self, v: u64) {
        self.value = v;
        println!("value set to {}", v);
        if v > 1000 {
            panic!("value too large");
        }
    }
}

fn helper_internal() {
    let x = 42;
    let y = x * 2;
    println!("{}", y);
    for i in 0..10 {
        println!("{}", i);
    }
}
"#;
        let result = try_signature_compact(rust_code);
        assert!(result.is_some(), "should extract Rust signatures");
        let extracted = result.unwrap();
        assert!(
            extracted.contains("pub fn compute_sum"),
            "should preserve fn signature"
        );
        assert!(
            extracted.contains("// ... body omitted ..."),
            "should replace body with placeholder"
        );
        assert!(
            extracted.contains("pub fn new"),
            "should preserve impl method signature"
        );
        assert!(
            extracted.len() < rust_code.len(),
            "should be shorter than original"
        );
    }

    #[test]
    fn test_signature_extract_non_rust() {
        let json = r#"{"key": "value", "nested": {"a": 1, "b": 2}}"#;
        assert!(
            try_signature_compact(json).is_none(),
            "JSON should not be treated as Rust"
        );

        let plain = "This is just some plain text with no code at all.\nIt has multiple lines.\nBut nothing resembling Rust.";
        assert!(
            try_signature_compact(plain).is_none(),
            "Plain text should return None"
        );
    }

    #[test]
    fn test_5_tier_selection() {
        // ≥85% → micro (most aggressive)
        let t = select_tier(0.90).unwrap();
        assert_eq!(t.preserve_recent, 2);
        assert_eq!(t.tool_result_max_chars, 200);

        let t = select_tier(0.85).unwrap();
        assert_eq!(t.preserve_recent, 2);

        // ≥70% → aggressive
        let t = select_tier(0.75).unwrap();
        assert_eq!(t.preserve_recent, 4);
        assert_eq!(t.tool_result_max_chars, 500);

        let t = select_tier(0.70).unwrap();
        assert_eq!(t.preserve_recent, 4);

        // ≥60% → moderate
        let t = select_tier(0.65).unwrap();
        assert_eq!(t.preserve_recent, 6);
        assert_eq!(t.tool_result_max_chars, 1000);
        assert_eq!(t.text_max_chars, 1500);

        let t = select_tier(0.60).unwrap();
        assert_eq!(t.preserve_recent, 6);

        // ≥30% → light
        let t = select_tier(0.45).unwrap();
        assert_eq!(t.preserve_recent, 8);
        assert_eq!(t.tool_result_max_chars, 3000);
        assert_eq!(t.text_max_chars, 4000);

        let t = select_tier(0.30).unwrap();
        assert_eq!(t.preserve_recent, 8);

        // ≥15% → history (least aggressive)
        let t = select_tier(0.20).unwrap();
        assert_eq!(t.preserve_recent, 6);
        assert_eq!(t.tool_result_max_chars, 1500);
        assert_eq!(t.text_max_chars, 2000);

        let t = select_tier(0.15).unwrap();
        assert_eq!(t.preserve_recent, 6);

        // Below 15% → no compaction
        assert!(select_tier(0.10).is_none());
        assert!(select_tier(0.0).is_none());
    }

    #[test]
    fn test_configurable_head_tail() {
        let content = "a".repeat(10_000);

        // Default 1/3 each
        let result_default = truncate_content(&content, 3000, None, None);
        assert!(result_default.starts_with(&"a".repeat(1000)));
        assert!(result_default.ends_with(&"a".repeat(1000)));
        assert!(result_default.contains("content truncated"));

        // Custom head=2000, tail=500
        let result_custom = truncate_content(&content, 3000, Some(2000), Some(500));
        let head_part: String = result_custom.chars().take(2000).collect();
        assert_eq!(head_part, "a".repeat(2000));
        assert!(result_custom.ends_with(&"a".repeat(500)));

        // Micro-style head=6000, tail=3000 on larger content
        let big_content = "b".repeat(20_000);
        let result_micro = truncate_content(&big_content, 10_000, Some(6000), Some(3000));
        assert!(result_micro.starts_with(&"b".repeat(6000)));
        assert!(result_micro.ends_with(&"b".repeat(3000)));
        assert!(result_micro.contains("content truncated"));
    }
}
