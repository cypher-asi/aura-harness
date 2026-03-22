//! Helper functions for the agent loop.

use aura_reasoner::{ContentBlock, Message, Role};

/// Push a warning message or replace the last warning if present.
///
/// This prevents accumulating multiple warning messages in the context.
pub fn push_or_replace_warning(messages: &mut Vec<Message>, warning: &str) {
    let is_warning = |text: &str| -> bool {
        text.starts_with("WARNING:")
            || text.starts_with("NOTE:")
            || text.starts_with("CRITICAL")
            || text.starts_with("STRONG WARNING:")
    };

    if let Some(last) = messages.last() {
        if last.role == Role::User {
            if let Some(ContentBlock::Text { text }) = last.content.first() {
                if is_warning(text) {
                    messages.pop();
                }
            }
        }
    }

    messages.push(Message::user(warning));
}

/// Normalize aura-app tool names to aura-harness names.
#[must_use]
pub fn normalize_tool_name(name: &str) -> &str {
    match name {
        "read_file" => "fs_read",
        "write_file" => "fs_write",
        "edit_file" => "fs_edit",
        "delete_file" => "fs_delete",
        "list_files" => "fs_ls",
        "find_files" => "fs_find",
        _ => name,
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
/// For `write_file`/`fs_write`: replaces content with path + byte size.
/// For `edit_file`/`fs_edit`: replaces `old_text`/`new_text` with path + edit description.
/// For other tools: returns `None` (input unchanged).
#[must_use]
pub fn summarize_write_input(
    tool_name: &str,
    input: &serde_json::Value,
) -> Option<serde_json::Value> {
    match tool_name {
        "fs_write" | "write_file" => {
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
                "_summarized": format!("Content: {} bytes written", content_len)
            }))
        }
        "fs_edit" | "edit_file" => {
            let path = input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let old_len = input
                .get("old_text")
                .or_else(|| input.get("old_string"))
                .and_then(|v| v.as_str())
                .map_or(0, str::len);
            let new_len = input
                .get("new_text")
                .or_else(|| input.get("new_string"))
                .and_then(|v| v.as_str())
                .map_or(0, str::len);
            Some(serde_json::json!({
                "path": path,
                "_summarized": format!("Edit: replaced {} chars with {} chars", old_len, new_len)
            }))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_tool_name_mapping() {
        assert_eq!(normalize_tool_name("read_file"), "fs_read");
        assert_eq!(normalize_tool_name("write_file"), "fs_write");
        assert_eq!(normalize_tool_name("edit_file"), "fs_edit");
        assert_eq!(normalize_tool_name("delete_file"), "fs_delete");
        assert_eq!(normalize_tool_name("list_files"), "fs_ls");
        assert_eq!(normalize_tool_name("find_files"), "fs_find");
    }

    #[test]
    fn test_normalize_tool_name_passthrough() {
        assert_eq!(normalize_tool_name("search_code"), "search_code");
        assert_eq!(normalize_tool_name("cmd_run"), "cmd_run");
        assert_eq!(normalize_tool_name("unknown_tool"), "unknown_tool");
    }

    #[test]
    fn test_push_or_replace_warning() {
        let mut messages = vec![Message::user("WARNING: old warning")];
        push_or_replace_warning(&mut messages, "WARNING: new warning");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text_content(), "WARNING: new warning");
    }

    #[test]
    fn test_summarize_write_file() {
        let input = serde_json::json!({
            "path": "src/main.rs",
            "content": "fn main() { println!(\"hello\"); }"
        });
        let result = summarize_write_input("write_file", &input).unwrap();
        assert_eq!(result["path"], "src/main.rs");
        assert!(result["_summarized"]
            .as_str()
            .unwrap()
            .contains("32 bytes written"));

        let result_fs = summarize_write_input("fs_write", &input).unwrap();
        assert_eq!(result_fs["path"], "src/main.rs");
        assert!(result_fs["_summarized"]
            .as_str()
            .unwrap()
            .contains("bytes written"));
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
        let summary = result["_summarized"].as_str().unwrap();
        assert!(summary.contains("replaced 16 chars with 3 chars"));

        let input_alt = serde_json::json!({
            "path": "src/lib.rs",
            "old_string": "abc",
            "new_string": "defgh"
        });
        let result_alt = summarize_write_input("fs_edit", &input_alt).unwrap();
        let summary_alt = result_alt["_summarized"].as_str().unwrap();
        assert!(summary_alt.contains("replaced 3 chars with 5 chars"));
    }

    #[test]
    fn test_summarize_read_file_unchanged() {
        let input = serde_json::json!({"path": "src/main.rs"});
        assert!(summarize_write_input("fs_read", &input).is_none());
        assert!(summarize_write_input("read_file", &input).is_none());
    }

    #[test]
    fn test_summarize_unknown_tool() {
        let input = serde_json::json!({"query": "some search"});
        assert!(summarize_write_input("search_code", &input).is_none());
        assert!(summarize_write_input("cmd_run", &input).is_none());
        assert!(summarize_write_input("totally_unknown", &input).is_none());
    }
}
