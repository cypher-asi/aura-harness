//! Message summary LLM-call prompts.
//!
//! A channel's HISTORY tab lists every message the user sent there. A
//! message too long for its row is shown as a one-line summary a small
//! model writes once; the harness caches it.

/// System prompt for the message summary call.
pub const MESSAGE_SUMMARY_SYSTEM_PROMPT: &str = "\
You summarize messages a user sent to a team of coding agents, for a \
narrow history list. Reply with only the summary: one line of at most 10 \
words in sentence case that says what the user asked for or said, such as \
\"Add a history tab to the channel panel\" or \"Ask RE36 to review the \
parser fix\". Keep @mentions of agents. No quotes, no trailing \
punctuation, no prefix like \"Summary:\".";

/// The user message for the message summary call: the user's message,
/// already shortened by the caller.
#[must_use]
pub fn message_summary_user_prompt(text: &str) -> String {
    format!("Message:\n{text}\n\nSummarize it in one line.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_prompt_holds_the_message() {
        let prompt = message_summary_user_prompt("@RE36 please review the parser");
        assert!(prompt.contains("Message:\n@RE36 please review the parser"));
    }

    #[test]
    fn system_prompt_asks_for_one_line() {
        assert!(MESSAGE_SUMMARY_SYSTEM_PROMPT.contains("one line"));
    }
}
