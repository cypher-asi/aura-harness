//! The criteria read out of the writer's reply, or why it holds none.

use std::borrow::Cow;

use aura_model::{Message, ModelResponse};
use serde_json::Value;

use super::{Criteria, Criterion, Kind, CRITERIA_TOOL, MAX_ITEMS};
use crate::request::{after_thinking, answered, Answered};

/// Characters kept of one criterion's text or reference.
pub(super) const MAX_TEXT: usize = 400;

/// The criteria in the writer's `response`; `None` when it gave none.
#[must_use]
pub fn criteria_of(response: &ModelResponse) -> Option<Criteria> {
    criteria_in(&response.message)
}

/// The criteria in the writer's `reply`: the arguments of its call to the
/// [`CRITERIA_TOOL`], or, when it called no tool, the JSON object in its
/// text. Items without a text or a known kind are dropped, repeats are
/// kept once and at most [`MAX_ITEMS`] stay. `None` when no item is left,
/// or it called another tool ([`none_because`] says which).
pub(super) fn criteria_in(reply: &Message) -> Option<Criteria> {
    let input = answer_in(reply).ok()?;
    from_input(&input)
}

/// Why [`criteria_of`] finds no criteria in `reply`, the writer's
/// message, in a few words.
#[must_use]
pub fn none_because(reply: &Message) -> String {
    let input = match answer_in(reply) {
        Ok(input) => input,
        Err(why) => return why,
    };
    let Some(listed) = listed_in(&input) else {
        return format!("its answer holds {}, not a list of items", holds(&input));
    };
    if listed.is_empty() {
        return "it listed no criteria".to_string();
    }
    format!(
        "none of its {} items has a text and a kind it was given (kinds: {})",
        listed.len(),
        kinds(&listed)
    )
}

/// What the writer answered with in `reply`: the arguments of its call to
/// the [`CRITERIA_TOOL`], or the JSON object in its text when it called
/// no tool.
///
/// # Errors
/// Says what the reply holds instead.
fn answer_in(reply: &Message) -> Result<Cow<'_, Value>, String> {
    match answered(reply, CRITERIA_TOOL) {
        Answered::Tool(input) => Ok(Cow::Borrowed(input)),
        Answered::Other(name) => Err(format!("it called `{name}`, not the criteria tool")),
        Answered::Text(text) if text.trim().is_empty() => Err("its reply was empty".to_string()),
        Answered::Text(text) => json_in(&text)
            .map(Cow::Owned)
            .ok_or_else(|| "its reply called no tool and holds no JSON object".to_string()),
    }
}

/// What `input`, an answer that lists no items, holds instead, in words.
fn holds(input: &Value) -> String {
    let Value::Object(fields) = input else {
        return shape(input).to_string();
    };
    match fields.get("items") {
        Some(items) => format!("`items` that is {}", shape(items)),
        None if fields.is_empty() => "nothing".to_string(),
        None => {
            let keys: Vec<&str> = fields.keys().map(String::as_str).collect();
            format!("only {}", keys.join(", "))
        }
    }
}

/// The kinds the `listed` items name, sorted and once each, or `none`.
fn kinds(listed: &[Value]) -> String {
    let mut kinds: Vec<&str> = listed
        .iter()
        .filter_map(|item| item.get("kind").and_then(Value::as_str))
        .collect();
    kinds.sort_unstable();
    kinds.dedup();
    if kinds.is_empty() {
        "none".to_string()
    } else {
        kinds.join(", ")
    }
}

/// What kind of JSON value `value` is, in words.
const fn shape(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

/// The items the writer listed in `input`. A model sometimes writes a
/// long nested argument as a string of JSON: the list is read from that
/// too, as is an answer that is such a string as a whole.
fn listed_in(input: &Value) -> Option<Cow<'_, [Value]>> {
    let parsed = |text: &str| serde_json::from_str::<Value>(text).ok();
    match input {
        Value::String(text) => {
            let whole = parsed(text).filter(|value| !value.is_string())?;
            listed_in(&whole).map(|items| Cow::Owned(items.into_owned()))
        }
        Value::Array(items) => Some(Cow::Borrowed(items)),
        Value::Object(fields) => match fields.get("items")? {
            Value::Array(items) => Some(Cow::Borrowed(items)),
            Value::String(text) => match parsed(text)? {
                Value::Array(items) => Some(Cow::Owned(items)),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// The JSON object in `text`: from its first `{` to its last `}`, after a
/// local model's `<think>` block.
fn json_in(text: &str) -> Option<Value> {
    let text = after_thinking(text);
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(text.get(start..=end)?).ok()
}

/// The criteria `input` lists, at most [`MAX_ITEMS`] and each text once;
/// `None` when it lists none with a text and a known kind.
fn from_input(input: &Value) -> Option<Criteria> {
    let mut items: Vec<Criterion> = Vec::new();
    for criterion in listed_in(input)?.iter().filter_map(criterion_in) {
        if items
            .iter()
            .any(|c| c.text.eq_ignore_ascii_case(&criterion.text))
        {
            continue;
        }
        items.push(criterion);
        if items.len() == MAX_ITEMS {
            break;
        }
    }
    let strict = match input.get("strict") {
        Some(Value::Bool(strict)) => *strict,
        Some(Value::String(said)) => said.trim().eq_ignore_ascii_case("true"),
        _ => false,
    };
    (!items.is_empty()).then_some(Criteria { items, strict })
}

/// The criterion `item` gives: its text, and a kind the writer was
/// given. `None` when it lacks either.
fn criterion_in(item: &Value) -> Option<Criterion> {
    let text = one_line(item.get("text"))?;
    let kind = item
        .get("kind")
        .and_then(Value::as_str)
        .and_then(|kind| Kind::from_name(&kind.trim().to_ascii_lowercase()))?;
    Some(Criterion {
        text,
        kind,
        reference: one_line(item.get("reference")),
    })
}

/// The text in `value`, on one line and at most [`MAX_TEXT`] characters
/// long; `None` when it holds none.
fn one_line(value: Option<&Value>) -> Option<String> {
    let mut text = value?
        .as_str()?
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if let Some((end, _)) = text.char_indices().nth(MAX_TEXT) {
        text.truncate(end);
    }
    (!text.is_empty()).then_some(text)
}
