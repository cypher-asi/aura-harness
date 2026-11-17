//! Chat title LLM-call prompts.
//!
//! After an agent's first turn the harness asks a small model for a short
//! title of what the chat is about, shown in the agents list in place of
//! the agent's placeholder name.

/// System prompt for the chat title call.
pub const CHAT_TITLE_SYSTEM_PROMPT: &str = "\
You name chat conversations between a user and a coding agent for a sidebar. \
Reply with only the title: 2 to 5 words in sentence case that name the \
user's task or topic, such as \"Task build issue\", \"Agent trace persistence \
issue\" or \"Companies section spacing\". No quotes, no trailing punctuation, \
no prefix like \"Title:\".";

/// The user message for the chat title call: the conversation's first
/// prompt and the agent's reply, already shortened by the caller.
#[must_use]
pub fn chat_title_user_prompt(prompt: &str, reply: &str) -> String {
    format!("User:\n{prompt}\n\nAgent:\n{reply}\n\nTitle this conversation.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_prompt_holds_both_sides() {
        let prompt = chat_title_user_prompt("fix the build", "Fixed the import.");
        assert!(prompt.contains("User:\nfix the build"));
        assert!(prompt.contains("Agent:\nFixed the import."));
    }
}
