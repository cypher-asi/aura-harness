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
    if reply.trim().is_empty() {
        return format!("User:\n{prompt}\n\nTitle this conversation.");
    }
    format!("User:\n{prompt}\n\nAgent:\n{reply}\n\nTitle this conversation.")
}

/// System prompt for naming a subtask a coding agent handed to a helper
/// agent. The name sits after the helper's code in a narrow tree row
/// (`RE36: Parser escapes`), so it is tighter than a chat title.
pub const SUBTASK_TITLE_SYSTEM_PROMPT: &str = "\
You label subtasks that a coding agent hands to helper agents, for a narrow \
sidebar tree. Reply with only the label: 1 to 3 words in sentence case naming \
the thing being worked on, such as \"Parser escapes\", \"Auth retries\", \
\"Inbox tests\" or \"Engine core\". Drop filler verbs like implement, add, \
fix, finish, update or create, and articles like \"the\". No quotes, no \
trailing punctuation, no prefix like \"Title:\".";

/// The user message for the subtask title call: the task prompt the
/// helper was given, already shortened by the caller.
#[must_use]
pub fn subtask_title_user_prompt(prompt: &str) -> String {
    format!("Task:\n{prompt}\n\nLabel this subtask.")
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

    #[test]
    fn subtask_prompt_holds_the_task() {
        let prompt = subtask_title_user_prompt("Implement the parser escapes");
        assert!(prompt.contains("Task:\nImplement the parser escapes"));
    }
}
