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
/// (`REN36: Parser escapes`), so it is tighter than a chat title.
pub const SUBTASK_TITLE_SYSTEM_PROMPT: &str = "\
You label subtasks that a coding agent hands to helper agents, for a narrow \
sidebar tree. Reply with only the label: 1 to 3 words in sentence case naming \
the thing being worked on, such as \"Parser escapes\", \"Auth retries\", \
\"Inbox tests\" or \"Engine core\". Drop filler verbs like implement, add, \
fix, finish, update or create, and articles like \"the\". Task prompts \
often open with boilerplate such as workspace paths, \"In the Rust \
workspace ...\", \"You are working in ...\" or rules about tools and \
committing: ignore it and name the work itself. No quotes, no trailing \
punctuation, no prefix like \"Title:\".";

/// The user message for the subtask title call: the task prompt the
/// helper was given, already shortened by the caller.
#[must_use]
pub fn subtask_title_user_prompt(prompt: &str) -> String {
    format!("Task:\n{prompt}\n\nLabel this subtask.")
}

/// System prompt for the subject of a thread of messages agents send
/// each other. Agents' messages open with workspace and tool boilerplate
/// that would otherwise become the subject, so the prompt says to skip it.
pub const MAIL_SUBJECT_SYSTEM_PROMPT: &str = "\
You write email-style subjects for messages coding agents send each other, \
shown in a narrow inbox list. Reply with only the subject: 3 to 6 words in \
sentence case naming what the message asks or reports, such as \"Inbox \
subject titles\", \"Review parser escapes\" or \"Benchmark results for \
Gate\". Ignore boilerplate such as workspace paths, \"Work ONLY in ...\" \
lines, and instructions about tools or committing. No quotes, no trailing \
punctuation, no prefix like \"Subject:\".";

/// The user message for the mail subject call: the thread's first
/// message, already shortened by the caller.
#[must_use]
pub fn mail_subject_user_prompt(text: &str) -> String {
    format!("Message:\n{text}\n\nWrite its subject.")
}

/// System prompt for the one-line summary of a long message the user
/// posted in a channel, shown in the channel's HISTORY list.
pub const POST_SUMMARY_SYSTEM_PROMPT: &str = "\
You summarize a message a user sent to a team of coding agents, for one row \
of a narrow list of the user's past messages. Reply with only the summary: \
one line of at most 10 words in sentence case saying what the user asked \
for or said, such as \"Make the footer links open in new tabs\" or \"Plan \
the parser rewrite, tests first\". Keep names of files, crates and agents. \
No quotes, no trailing period, no prefix like \"Summary:\".";

/// The user message for the post summary call: the post, already
/// shortened by the caller.
#[must_use]
pub fn post_summary_user_prompt(text: &str) -> String {
    format!("Message:\n{text}\n\nSummarize it in one line.")
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

    #[test]
    fn mail_subject_prompt_holds_the_message() {
        let prompt = mail_subject_user_prompt("Please review the parser escapes");
        assert!(prompt.contains("Message:\nPlease review the parser escapes"));
        assert!(prompt.ends_with("Write its subject."));
    }
}
