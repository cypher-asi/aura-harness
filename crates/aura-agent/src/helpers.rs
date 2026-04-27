//! Helper functions for the agent loop.

use aura_reasoner::{ContentBlock, Message, Role};
use serde_json::Value;
use std::path::Path;

use crate::types::{FileChange, FileChangeKind};

/// Append a warning as a text block to the last user message, or push a new
/// user message if the last message isn't a user message.
///
/// This is safe to call after `tool_result` messages because it appends to
/// the existing user message rather than inserting a new one that would
/// break the `tool_use/tool_result` adjacency required by Anthropic.
pub fn append_warning(messages: &mut Vec<Message>, warning: &str) {
    if let Some(last) = messages.last_mut() {
        if last.role == Role::User {
            last.content.push(ContentBlock::Text {
                text: warning.to_string(),
            });
            return;
        }
    }
    messages.push(Message::user(warning));
}

/// Strip property descriptions from tool definitions to reduce token usage.
pub fn compact_tools(tools: &mut [aura_reasoner::ToolDefinition]) {
    for tool in tools {
        if let Some(props) = tool.input_schema.get_mut("properties") {
            if let Some(obj) = props.as_object_mut() {
                for (_, prop_schema) in obj.iter_mut() {
                    if let Some(inner) = prop_schema.as_object_mut() {
                        inner.remove("description");
                    }
                }
            }
        }
    }
}

/// Check if a tool name is a write tool (mutation).
#[must_use]
pub fn is_write_tool(name: &str) -> bool {
    crate::constants::WRITE_TOOLS.contains(&name)
}

/// Check if a tool name is an exploration tool (read-only).
#[must_use]
pub fn is_exploration_tool(name: &str) -> bool {
    crate::constants::EXPLORATION_TOOLS.contains(&name)
}

/// Summarize write tool inputs to save context tokens.
///
/// For `write_file`: replaces content with a sentinel under the real `content`
/// key. For `edit_file`: replaces `old_text/new_text` (or string aliases) with
/// sentinels under the real keys. Keeping the schema-shaped keys prevents the
/// model from learning invalid `_summarized` argument shapes from its own
/// history while still avoiding full-content replay in context.
/// For other tools: returns `None` (input unchanged).
#[must_use]
pub fn summarize_write_input(
    tool_name: &str,
    input: &serde_json::Value,
) -> Option<serde_json::Value> {
    match tool_name {
        "write_file" => {
            let path = input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let content_len = input
                .get("content")
                .and_then(|v| v.as_str())
                .map_or(0, str::len);
            Some(serde_json::json!({
                "path": path,
                "content": format!("<<<AURA_ELIDED_CONTENT::{content_len}_bytes>>>")
            }))
        }
        "edit_file" => {
            let path = input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let old_key = if input.get("old_string").is_some() {
                "old_string"
            } else {
                "old_text"
            };
            let new_key = if input.get("new_string").is_some() {
                "new_string"
            } else {
                "new_text"
            };
            let old_len = input
                .get(old_key)
                .and_then(|v| v.as_str())
                .map_or(0, str::len);
            let new_len = input
                .get(new_key)
                .and_then(|v| v.as_str())
                .map_or(0, str::len);
            let mut summarized = serde_json::Map::new();
            summarized.insert("path".to_string(), serde_json::json!(path));
            summarized.insert(
                old_key.to_string(),
                serde_json::json!(format!("<<<AURA_ELIDED_OLD::{old_len}_chars>>>")),
            );
            summarized.insert(
                new_key.to_string(),
                serde_json::json!(format!("<<<AURA_ELIDED_NEW::{new_len}_chars>>>")),
            );
            Some(serde_json::Value::Object(summarized))
        }
        _ => None,
    }
}

/// Collapse oversized repeated cache hits for read-only tools.
///
/// First-time tool outputs stay untouched. This only shapes large results that
/// the model has already seen earlier in the same run, which helps limit prompt
/// growth from repeated reads and searches without weakening the initial result.
#[must_use]
pub fn summarize_cached_tool_result(
    tool_name: &str,
    input: &Value,
    content: &str,
) -> Option<String> {
    if std::env::var("AURA_DISABLE_CACHED_RESULT_SHAPING")
        .ok()
        .as_deref()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
    {
        return None;
    }

    let (reuse_threshold, max_chars, head_chars, tail_chars) = match tool_name {
        "read_file" => (8_000, 4_000, 3_000, 500),
        "search_code" => (4_000, 2_000, 1_500, 250),
        "list_files" | "find_files" => (2_500, 1_200, 900, 150),
        "stat_file" => (1_500, 900, 650, 100),
        _ => return None,
    };

    if content.len() <= reuse_threshold {
        return None;
    }

    let descriptor = cached_tool_descriptor(input);
    let truncated =
        crate::compaction::truncate_content(content, max_chars, Some(head_chars), Some(tail_chars));
    Some(format!(
        "Cached result reused from earlier identical `{tool_name}` call{descriptor}. Full output was {} chars.\n\n{truncated}",
        content.len()
    ))
}

fn cached_tool_descriptor(input: &Value) -> String {
    let mut parts = Vec::new();

    if let Some(path) = input.get("path").and_then(|v| v.as_str()) {
        parts.push(format!("path={path}"));
    }
    if let Some(pattern) = input.get("pattern").and_then(|v| v.as_str()) {
        parts.push(format!("pattern={pattern}"));
    }
    if let Some(query) = input.get("query").and_then(|v| v.as_str()) {
        parts.push(format!("query={query}"));
    }
    if let Some(start_line) = input.get("start_line").and_then(|v| v.as_u64()) {
        parts.push(format!("start_line={start_line}"));
    }
    if let Some(end_line) = input.get("end_line").and_then(|v| v.as_u64()) {
        parts.push(format!("end_line={end_line}"));
    }

    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(", "))
    }
}

/// Compute (`lines_added`, `lines_removed`) between two strings using a
/// line-wise diff.
///
/// Returns `(added, removed)`. Both counts are clamped to `u32` —
/// realistic file edits never approach 4B lines so saturating cast is
/// safe; even pathological inputs are bounded by `usize::MAX` and we
/// just lose precision past `u32::MAX` rather than panic.
fn diff_line_counts(old_text: &str, new_text: &str) -> (u32, u32) {
    use similar::{ChangeTag, TextDiff};
    let diff = TextDiff::from_lines(old_text, new_text);
    let mut added: u32 = 0;
    let mut removed: u32 = 0;
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => added = added.saturating_add(1),
            ChangeTag::Delete => removed = removed.saturating_add(1),
            ChangeTag::Equal => {}
        }
    }
    (added, removed)
}

/// Infer file mutations for a successful write tool call.
///
/// `lines_added` / `lines_removed` are populated for `edit_file` (which
/// carries both `old_text` and `new_text` in its input). For
/// `write_file` and `delete_file` the counts stay at 0 because the
/// pre-mutation file content is not available at this layer — by the
/// time the tool result reaches us the write has already happened or
/// the file is gone. Downstream consumers must treat 0 as "unknown".
#[must_use]
pub fn infer_file_changes(
    tool_name: &str,
    input: &serde_json::Value,
    base_path: Option<&Path>,
) -> Vec<FileChange> {
    let Some(path) = input.get("path").and_then(|v| v.as_str()) else {
        return Vec::new();
    };

    let existed_before = base_path.map(|base| base.join(path).exists());
    let kind = match tool_name {
        "write_file" => {
            if matches!(existed_before, Some(false)) {
                FileChangeKind::Create
            } else {
                FileChangeKind::Modify
            }
        }
        "edit_file" => FileChangeKind::Modify,
        "delete_file" => {
            if matches!(existed_before, Some(false)) {
                return Vec::new();
            }
            FileChangeKind::Delete
        }
        _ => return Vec::new(),
    };

    let (lines_added, lines_removed) = if tool_name == "edit_file" {
        let old_text = input.get("old_text").and_then(|v| v.as_str()).unwrap_or("");
        let new_text = input.get("new_text").and_then(|v| v.as_str()).unwrap_or("");
        diff_line_counts(old_text, new_text)
    } else {
        (0, 0)
    };

    vec![FileChange {
        path: path.to_string(),
        kind,
        lines_added,
        lines_removed,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compact_tools_strips_descriptions() {
        let mut tools = vec![aura_reasoner::ToolDefinition::new(
            "test",
            "A tool",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The file path"
                    }
                }
            }),
        )];
        compact_tools(&mut tools);
        let props = tools[0].input_schema["properties"]["path"]
            .as_object()
            .unwrap();
        assert!(!props.contains_key("description"));
        assert!(props.contains_key("type"));
    }

    #[test]
    fn test_append_warning_to_existing_user_message() {
        let mut messages = vec![Message::user("hello")];
        append_warning(&mut messages, "WARNING: something");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content.len(), 2);
    }

    #[test]
    fn test_append_warning_after_assistant() {
        let mut messages = vec![Message::assistant("response")];
        append_warning(&mut messages, "WARNING: something");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].role, Role::User);
    }

    #[test]
    fn test_summarize_write_file() {
        let input = serde_json::json!({
            "path": "src/main.rs",
            "content": "fn main() { println!(\"hello\"); }"
        });
        let result = summarize_write_input("write_file", &input).unwrap();
        assert_eq!(result["path"], "src/main.rs");
        assert_eq!(result["content"], "<<<AURA_ELIDED_CONTENT::32_bytes>>>");
        assert!(result.get("_summarized").is_none());

        let result2 = summarize_write_input("write_file", &input).unwrap();
        assert_eq!(result2["path"], "src/main.rs");
        assert!(result2["content"]
            .as_str()
            .unwrap()
            .contains("AURA_ELIDED_CONTENT"));
    }

    #[test]
    fn test_summarize_edit_file() {
        let input = serde_json::json!({
            "path": "src/lib.rs",
            "old_text": "old content here",
            "new_text": "new"
        });
        let result = summarize_write_input("edit_file", &input).unwrap();
        assert_eq!(result["path"], "src/lib.rs");
        assert_eq!(result["old_text"], "<<<AURA_ELIDED_OLD::16_chars>>>");
        assert_eq!(result["new_text"], "<<<AURA_ELIDED_NEW::3_chars>>>");
        assert!(result.get("_summarized").is_none());

        let input_alt = serde_json::json!({
            "path": "src/lib.rs",
            "old_string": "abc",
            "new_string": "defgh"
        });
        let result2 = summarize_write_input("edit_file", &input_alt).unwrap();
        assert_eq!(result2["old_string"], "<<<AURA_ELIDED_OLD::3_chars>>>");
        assert_eq!(result2["new_string"], "<<<AURA_ELIDED_NEW::5_chars>>>");
        assert!(result2.get("old_text").is_none());
        assert!(result2.get("new_text").is_none());
    }

    #[test]
    fn test_summarize_read_file_unchanged() {
        let input = serde_json::json!({"path": "src/main.rs"});
        assert!(summarize_write_input("read_file", &input).is_none());
    }

    #[test]
    fn test_summarize_unknown_tool() {
        let input = serde_json::json!({"query": "some search"});
        assert!(summarize_write_input("search_code", &input).is_none());
        assert!(summarize_write_input("run_command", &input).is_none());
        assert!(summarize_write_input("totally_unknown", &input).is_none());
    }

    #[test]
    fn test_summarize_cached_tool_result_for_large_read_file() {
        let input = serde_json::json!({"path": "src/lib.rs"});
        let content = "a".repeat(9_000);
        let summary = summarize_cached_tool_result("read_file", &input, &content).unwrap();
        assert!(summary.contains("Cached result reused"));
        assert!(summary.contains("path=src/lib.rs"));
        assert!(summary.contains("Full output was 9000 chars"));
        assert!(summary.contains("truncated"));
        assert!(summary.len() < content.len());
    }

    #[test]
    fn test_summarize_cached_tool_result_cuts_large_read_file_footprint_substantially() {
        let input = serde_json::json!({"path": "src/lib.rs"});
        let content = "a".repeat(9_000);
        let summary = summarize_cached_tool_result("read_file", &input, &content).unwrap();
        let saved_chars = content.len() - summary.len();
        assert!(summary.len() <= 4_300, "summary should stay compact");
        assert!(
            saved_chars >= 4_500,
            "expected at least 4.5k chars saved, got {saved_chars}"
        );
    }

    #[test]
    fn test_summarize_cached_tool_result_leaves_small_result_unchanged() {
        let input = serde_json::json!({"path": "src/lib.rs"});
        let content = "fn main() {}\n";
        assert!(summarize_cached_tool_result("read_file", &input, content).is_none());
    }

    #[test]
    fn test_summarize_cached_tool_result_ignores_unknown_tools() {
        let input = serde_json::json!({"command": "pwd"});
        let content = "x".repeat(10_000);
        assert!(summarize_cached_tool_result("run_command", &input, &content).is_none());
    }

    #[test]
    fn test_summarize_cached_tool_result_cuts_large_search_code_footprint() {
        let input = serde_json::json!({"pattern": "fn main", "path": "src"});
        let content = "b".repeat(6_000);
        let summary = summarize_cached_tool_result("search_code", &input, &content).unwrap();
        let saved_chars = content.len() - summary.len();
        assert!(summary.len() <= 2_300, "summary should stay compact");
        assert!(
            saved_chars >= 3_500,
            "expected at least 3.5k chars saved, got {saved_chars}"
        );
    }

    #[test]
    fn test_summarize_cached_tool_result_cuts_large_list_files_footprint() {
        let input = serde_json::json!({"path": "."});
        let content = "c".repeat(3_000);
        let summary = summarize_cached_tool_result("list_files", &input, &content).unwrap();
        let saved_chars = content.len() - summary.len();
        assert!(summary.len() <= 1_400, "summary should stay compact");
        assert!(
            saved_chars >= 1_500,
            "expected at least 1.5k chars saved, got {saved_chars}"
        );
    }

    #[test]
    fn test_infer_file_changes_write_create_without_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let input = serde_json::json!({"path": "src/new.rs"});
        let changes = infer_file_changes("write_file", &input, Some(dir.path()));
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "src/new.rs");
        assert!(matches!(changes[0].kind, FileChangeKind::Create));
    }

    #[test]
    fn test_infer_file_changes_write_modify_with_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("src/lib.rs");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "old").unwrap();

        let input = serde_json::json!({"path": "src/lib.rs"});
        let changes = infer_file_changes("write_file", &input, Some(dir.path()));
        assert_eq!(changes.len(), 1);
        assert!(matches!(changes[0].kind, FileChangeKind::Modify));
    }

    #[test]
    fn test_infer_file_changes_write_defaults_to_modify_without_base_path() {
        let input = serde_json::json!({"path": "src/lib.rs"});
        let changes = infer_file_changes("write_file", &input, None);
        assert_eq!(changes.len(), 1);
        assert!(matches!(changes[0].kind, FileChangeKind::Modify));
    }

    #[test]
    fn diff_line_counts_pure_insert() {
        let (added, removed) = diff_line_counts("a\nb\n", "a\nb\nc\nd\n");
        assert_eq!(added, 2);
        assert_eq!(removed, 0);
    }

    #[test]
    fn diff_line_counts_pure_delete() {
        let (added, removed) = diff_line_counts("a\nb\nc\nd\n", "a\nb\n");
        assert_eq!(added, 0);
        assert_eq!(removed, 2);
    }

    #[test]
    fn diff_line_counts_mixed_replace() {
        // Replacing a single line counts as one insert + one delete.
        let (added, removed) = diff_line_counts("a\nold\nc\n", "a\nnew\nc\n");
        assert_eq!(added, 1);
        assert_eq!(removed, 1);
    }

    #[test]
    fn diff_line_counts_identical_strings() {
        let (added, removed) = diff_line_counts("same\nlines\n", "same\nlines\n");
        assert_eq!(added, 0);
        assert_eq!(removed, 0);
    }

    #[test]
    fn test_infer_file_changes_edit_populates_line_counts() {
        let input = serde_json::json!({
            "path": "src/lib.rs",
            "old_text": "fn old() {}\n",
            "new_text": "fn new() {}\nfn extra() {}\n",
        });
        let changes = infer_file_changes("edit_file", &input, None);
        assert_eq!(changes.len(), 1);
        assert!(matches!(changes[0].kind, FileChangeKind::Modify));
        // One line replaced + one new line appended → 2 inserts, 1 delete.
        assert_eq!(changes[0].lines_added, 2);
        assert_eq!(changes[0].lines_removed, 1);
    }

    #[test]
    fn test_infer_file_changes_edit_missing_inputs_yields_zero() {
        let input = serde_json::json!({"path": "src/lib.rs"});
        let changes = infer_file_changes("edit_file", &input, None);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].lines_added, 0);
        assert_eq!(changes[0].lines_removed, 0);
    }

    #[test]
    fn test_infer_file_changes_write_leaves_line_counts_zero() {
        // write_file can't compute pre-content from input alone; counts
        // stay at 0 so downstream consumers know the diff is unknown.
        let input = serde_json::json!({"path": "src/new.rs", "content": "a\nb\nc\n"});
        let changes = infer_file_changes("write_file", &input, None);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].lines_added, 0);
        assert_eq!(changes[0].lines_removed, 0);
    }
}
