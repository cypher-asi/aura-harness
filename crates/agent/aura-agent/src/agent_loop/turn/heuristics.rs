//! Text heuristics over the conversation that decide whether a turn
//! that stopped should be re-prompted.

use aura_model::{ContentBlock, Message, Role};

/// The last assistant message's text blocks joined by newlines and
/// lowercased, or `None` when there is no assistant message.
fn last_assistant_text_lower(messages: &[Message]) -> Option<String> {
    let message = messages.iter().rev().find(|m| m.role == Role::Assistant)?;
    let text = message
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase();
    Some(text)
}

/// Whether the last assistant message asks the user something or says
/// it is blocked, in which case ending the turn is the right call.
pub(super) fn waits_on_user(messages: &[Message]) -> bool {
    let Some(text) = last_assistant_text_lower(messages) else {
        return false;
    };
    text.trim_end().ends_with('?')
        || [
            "blocked",
            "need your",
            "waiting for",
            "approval",
            "permission",
            "let me know",
            "would you like",
            "should i",
        ]
        .iter()
        .any(|marker| text.contains(marker))
}

pub(super) fn is_planning_request(messages: &[Message]) -> bool {
    let text = messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .map(|m| {
            m.content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
        .to_ascii_lowercase();
    [
        "plan",
        "outline",
        "explain",
        "how ",
        "what ",
        "draft a plan",
        "can you explain",
    ]
    .iter()
    .any(|prefix| text.trim_start().starts_with(prefix))
        || [
            "don't implement",
            "do not implement",
            "planning only",
            "plan only",
        ]
        .iter()
        .any(|marker| text.contains(marker))
}

pub(super) fn ends_with_unfinished_action(messages: &[Message]) -> bool {
    let Some(text) = last_assistant_text_lower(messages) else {
        return false;
    };
    if defers_to_user(&text) {
        return false;
    }
    let sentence = last_sentence(&text);
    if sentence == "doing it now" {
        return true;
    }
    promises_action(sentence)
}

/// Whether the text asks something, reports a blocker or defers the
/// work, so a trailing "I'll ..." is not an unfinished action.
fn defers_to_user(text: &str) -> bool {
    text.contains('?')
        || [
            "please ",
            "blocked",
            "cannot ",
            "can't ",
            "need your",
            "waiting for",
            "tomorrow",
            "later",
            "next build",
            "if you",
            "once you",
            "when you",
            "after you",
            "approval",
            "permission",
        ]
        .iter()
        .any(|marker| text.contains(marker))
}

fn last_sentence(text: &str) -> &str {
    text.trim()
        .trim_end_matches(['.', '!'])
        .rsplit(['\n'])
        .next()
        .unwrap_or("")
        .rsplit(". ")
        .next()
        .unwrap_or("")
        .trim()
}

/// Whether `sentence` promises an immediate tool-backed action
/// ("i'll read ...", "i will fix ...").
fn promises_action(sentence: &str) -> bool {
    let Some(action) = ["i'll ", "i’ll ", "i will "]
        .iter()
        .find_map(|prefix| sentence.strip_prefix(prefix))
    else {
        return false;
    };
    [
        "read ",
        "check ",
        "find ",
        "locate ",
        "run ",
        "test ",
        "verify ",
        "edit ",
        "write ",
        "patch ",
        "fix ",
        "remove ",
        "delete ",
        "update ",
        "implement ",
        "search ",
    ]
    .iter()
    .any(|verb| action.starts_with(verb))
}
