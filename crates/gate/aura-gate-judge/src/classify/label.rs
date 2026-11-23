//! The label in the classifier's reply: the arguments of its call to the
//! label tool, or the words of a text answer.

use aura_core_types::{Difficulty, PromptCategory};
use aura_model::{Message, ModelResponse};
use serde_json::Value;

use super::{Label, LABEL_TOOL};
use crate::request::{after_thinking, answered, calls, Answered};

/// What the classifier's `response` says; `None` when it names no
/// category or no difficulty.
#[must_use]
pub fn label_of(response: &ModelResponse) -> Option<Label> {
    label_in(&response.message)
}

/// The label in the classifier's `reply`: the arguments of its call to
/// the [`LABEL_TOOL`], or, when it called no tool, its text. `None` when
/// the call's arguments are not one category and one difficulty, or it
/// called another tool.
pub(super) fn label_in(reply: &Message) -> Option<Label> {
    match answered(reply, LABEL_TOOL) {
        Answered::Tool(input) => called_with(input),
        Answered::Text(text) => parse_label(&text),
        Answered::Other(_) => None,
    }
}

/// The label `input` gives, the arguments of a call to the
/// [`LABEL_TOOL`].
fn called_with(input: &Value) -> Option<Label> {
    let field = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .map(|value| value.trim().to_ascii_lowercase())
    };
    Some(Label {
        category: PromptCategory::from_name(&field("category")?)?,
        difficulty: Difficulty::from_name(&field("difficulty")?)?,
    })
}

/// The classifier's `reply` as said when it gave no label: its text and
/// the tools it called with their arguments.
#[must_use]
pub fn reply_shown(reply: &Message) -> String {
    let text = reply.text_content();
    let called = calls(reply).map(|(name, input)| format!("{name}({input})"));
    std::iter::once(text.trim().to_string())
        .chain(called)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The label in `reply`, a text answer: its first category and its first
/// difficulty. `None` when it lacks either.
pub(super) fn parse_label(reply: &str) -> Option<Label> {
    Some(Label {
        category: parse(reply)?,
        difficulty: parse_difficulty(reply)?,
    })
}

/// The first category named in `reply`, if any.
pub(super) fn parse(reply: &str) -> Option<PromptCategory> {
    first_named(reply, PromptCategory::from_name)
}

/// The first difficulty named in `reply`, if any.
pub(super) fn parse_difficulty(reply: &str) -> Option<Difficulty> {
    first_named(reply, Difficulty::from_name)
}

/// The first word of `reply`'s answer that `named` reads as a name. The
/// words are lowercase, with `-` read as `_`, and follow a local model's
/// `<think>` block, since it may weigh several labels before answering.
fn first_named<T>(reply: &str, named: impl Fn(&str) -> Option<T>) -> Option<T> {
    after_thinking(reply)
        .to_ascii_lowercase()
        .replace('-', "_")
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty())
        .find_map(named)
}
