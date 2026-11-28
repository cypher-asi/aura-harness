//! Bounding and shaping what the Runpod tools return to the model.

use serde_json::{Map, Value};

/// Largest tool result, in bytes.
pub const MAX_OUTPUT_BYTES: usize = 32 * 1024;

/// Items a list returns when the call doesn't say.
pub(crate) const DEFAULT_LIST_ITEMS: usize = 20;

/// Most items a list returns.
pub(crate) const MAX_LIST_ITEMS: usize = 100;

/// `value` as pretty JSON, cut at [`MAX_OUTPUT_BYTES`] with a note saying
/// how much was kept.
pub(crate) fn render(value: &Value) -> String {
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    cap_bytes(&text, MAX_OUTPUT_BYTES)
}

/// `text` cut to at most `max` bytes on a character boundary, with a note
/// when anything was cut.
pub(crate) fn cap_bytes(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n... [output trimmed: {end} of {} bytes shown]",
        &text[..end],
        text.len()
    )
}

/// `text` cut to at most `max` characters, with `...` when cut.
pub(crate) fn truncate_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}...", &text[..end]),
        None => text.to_string(),
    }
}

/// An object of `fields`, each `(output name, JSON pointer into value)`.
/// Missing fields are left out rather than shown as `null`.
pub(crate) fn pick(value: &Value, fields: &[(&str, &str)]) -> Value {
    let mut out = Map::new();
    for (name, pointer) in fields {
        if let Some(found) = value.pointer(pointer).filter(|v| !v.is_null()) {
            out.insert((*name).to_string(), found.clone());
        }
    }
    Value::Object(out)
}

/// The array at `pointer` in `value`, each item shaped by `shape`, kept to
/// `limit` items, with the total when some were left out.
pub(crate) fn list(
    value: &Value,
    pointer: &str,
    key: &str,
    limit: usize,
    shape: impl Fn(&Value) -> Value,
) -> Map<String, Value> {
    let items = value
        .pointer(pointer)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut out = Map::new();
    out.insert(
        key.to_string(),
        Value::Array(items.iter().take(limit).map(shape).collect()),
    );
    if items.len() > limit {
        out.insert("shown".to_string(), Value::from(limit));
        out.insert("total".to_string(), Value::from(items.len()));
    }
    out
}

/// The `limit` argument clamped to 1..=[`MAX_LIST_ITEMS`].
pub(crate) fn list_limit(asked: Option<u64>) -> usize {
    asked
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(DEFAULT_LIST_ITEMS)
        .clamp(1, MAX_LIST_ITEMS)
}
