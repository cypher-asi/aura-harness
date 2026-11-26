//! Character-size estimates for messages and JSON values.

use aura_config::CHARS_PER_TOKEN;
use aura_model::{ContentBlock, Message, ToolResultContent};

/// Estimate total character count of messages.
#[must_use]
pub fn estimate_message_chars(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|m| {
            m.content
                .iter()
                .map(|block| match block {
                    ContentBlock::Text { text } => text.len(),
                    ContentBlock::Thinking { thinking, .. } => thinking.len(),
                    ContentBlock::ToolUse { input, .. } => json_len(input),
                    ContentBlock::ToolResult { content, .. } => match content {
                        ToolResultContent::Text(t) => t.len(),
                        ToolResultContent::Json(v) => json_len(v),
                    },
                    ContentBlock::Image { source } => source.data.len(),
                })
                .sum::<usize>()
        })
        .sum()
}

/// Length of `value` written as compact JSON, without writing it out.
pub(crate) fn json_len(value: &serde_json::Value) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 += buf.len();
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    serde_json::to_writer(&mut count, value).map_or(0, |()| count.0)
}

/// Convert message characters to the existing heuristic token estimate.
#[must_use]
pub fn message_chars_to_tokens(chars: usize) -> u64 {
    #[allow(clippy::cast_possible_truncation)]
    {
        (chars / CHARS_PER_TOKEN) as u64
    }
}
