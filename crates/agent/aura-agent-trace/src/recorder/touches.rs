//! Pure helpers deriving step fields from tool calls.

use aura_core_types::trace::FileTouch;
use serde_json::Value;

/// Files a tool call read or wrote, derived from its input.
#[must_use]
pub fn file_touches(tool: &str, input: &Value, is_error: bool) -> Vec<FileTouch> {
    let Some(path) = input.get("path").and_then(Value::as_str) else {
        return Vec::new();
    };
    let kind = match tool {
        "read_file" | "stat_file" => "read",
        "write_file" => "create",
        "edit_file" | "multi_edit" | "apply_patch" => "modify",
        "delete_file" => "delete",
        _ => return Vec::new(),
    };
    if is_error && kind != "read" {
        return Vec::new();
    }
    let lines = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .map_or(0, |s| u32::try_from(s.lines().count()).unwrap_or(u32::MAX))
    };
    let (added, removed) = match tool {
        "edit_file" => (lines("new_text"), lines("old_text")),
        "write_file" => (lines("content"), 0),
        _ => (0, 0),
    };
    vec![FileTouch {
        path: path.to_string(),
        kind: kind.to_string(),
        added,
        removed,
    }]
}

/// First `max` characters of `text`, with an ellipsis when cut.
#[must_use]
pub fn excerpt(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().nth(max).is_some() {
        out.push('…');
    }
    out
}
