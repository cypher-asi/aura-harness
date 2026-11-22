//! Reading a model's reply to one of Gate's requests.

use aura_model::{ContentBlock, Message};
use serde_json::Value;

/// What a model answered with when it was offered one tool.
pub(crate) enum Answered<'a> {
    /// The input of its call to that tool.
    Tool(&'a Value),
    /// The name of another tool it called first.
    Other(&'a str),
    /// Its text: it called no tool.
    Text(String),
}

/// What `reply` answers with when it was offered `tool`: its first tool
/// call, or its text when it called none.
pub(crate) fn answered<'a>(reply: &'a Message, tool: &str) -> Answered<'a> {
    match calls(reply).next() {
        Some((name, input)) if name == tool => Answered::Tool(input),
        Some((name, _)) => Answered::Other(name),
        None => Answered::Text(reply.text_content()),
    }
}

/// The tools `reply` calls, in order, with their inputs.
pub(crate) fn calls(reply: &Message) -> impl Iterator<Item = (&str, &Value)> {
    reply.content.iter().filter_map(|block| match block {
        ContentBlock::ToolUse { name, input, .. } => Some((name.as_str(), input)),
        _ => None,
    })
}

/// `reply` after its last `</think>`: a local model may think aloud
/// before it answers, and weigh several answers as it does.
pub(crate) fn after_thinking(reply: &str) -> &str {
    reply
        .rsplit_once("</think>")
        .map_or(reply, |(_, after)| after)
}
