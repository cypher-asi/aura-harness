//! Assertion/extraction helpers over turn message streams, plus
//! filesystem helpers for agent workspaces.

use std::path::{Path, PathBuf};

use serde_json::Value;

// ============================================================================
// Assertion / extraction helpers
// ============================================================================

/// Extract all tool names used in a turn's message stream.
pub fn tool_names_used(messages: &[Value]) -> Vec<String> {
    messages
        .iter()
        .filter(|m| m["type"] == "tool_use_start")
        .filter_map(|m| m["name"].as_str().map(String::from))
        .collect()
}

/// Concatenate all text_delta content in a turn.
pub fn collect_text(messages: &[Value]) -> String {
    messages
        .iter()
        .filter(|m| m["type"] == "text_delta")
        .filter_map(|m| m["text"].as_str())
        .collect()
}

/// Check that a turn ended with a given stop_reason.
pub fn assert_stop_reason(messages: &[Value], expected: &str) {
    let end = messages
        .iter()
        .find(|m| m["type"] == "assistant_message_end");
    assert!(end.is_some(), "no assistant_message_end found");
    assert_eq!(
        end.unwrap()["stop_reason"].as_str().unwrap(),
        expected,
        "unexpected stop_reason"
    );
}

/// Check that a turn contains a tool_result with is_error for a given tool.
pub fn has_tool_error(messages: &[Value], tool_name: &str) -> bool {
    messages.iter().any(|m| {
        m["type"] == "tool_result"
            && m["name"] == tool_name
            && m["is_error"].as_bool() == Some(true)
    })
}

// ============================================================================
// Filesystem helpers
// ============================================================================

/// Find a file by name anywhere under a directory tree (deepest match first).
pub fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = find_file(&path, name) {
                    return Some(found);
                }
            }
        }
    }
    if dir.join(name).exists() {
        return Some(dir.join(name));
    }
    None
}

/// Find the agent subdirectory created by the session under the workspace.
pub fn find_agent_dir(ws_path: &Path) -> Option<PathBuf> {
    for entry in std::fs::read_dir(ws_path).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            return Some(path);
        }
    }
    None
}

/// Place a file in the agent workspace directory (where tools operate).
pub fn place_file_in_agent_dir(ws_path: &Path, name: &str, content: &str) {
    if let Some(agent_dir) = find_agent_dir(ws_path) {
        std::fs::write(agent_dir.join(name), content).unwrap();
    } else {
        std::fs::write(ws_path.join(name), content).unwrap();
    }
}
