//! Helper functions for the agent loop.

use aura_reasoner::{ContentBlock, Message, Role};

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
/// For `write_file`: replaces content with path + byte size.
/// For `edit_file`: replaces `old_text/new_text` with path + edit description.
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
                "_summarized": format!("Content: {} bytes written", content_len)
            }))
        }
        "edit_file" => {
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
        assert!(result["_summarized"]
            .as_str()
            .unwrap()
            .contains("32 bytes written"));

        let result2 = summarize_write_input("write_file", &input).unwrap();
        assert_eq!(result2["path"], "src/main.rs");
        assert!(result2["_summarized"]
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
        let result2 = summarize_write_input("edit_file", &input_alt).unwrap();
        let summary2 = result2["_summarized"].as_str().unwrap();
        assert!(summary2.contains("replaced 3 chars with 5 chars"));
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
}
