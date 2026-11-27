//! What one turn of an agent did, read from its conversation: the plan it
//! wrote and the pushes it made, by the tools that ran.

use super::observe::is_push_tool;
use aura_exec_merge_queue::QueuedPush;
use aura_exec_tools::plan_tools::{plan_path_from_output, CREATE_PLAN_TOOL_NAME};
use aura_model::{ContentBlock, Message, Role, ToolResultContent};
use std::collections::HashMap;

/// Each successful tool call of the last turn in `messages` (since the
/// last prompt): its tool's name and its result.
pub(crate) fn turn_calls(messages: &[Message]) -> Vec<(String, String)> {
    let start = messages
        .iter()
        .rposition(|message| {
            message.role == Role::User
                && message
                    .content
                    .iter()
                    .any(|block| matches!(block, ContentBlock::Text { .. }))
        })
        .unwrap_or(0);
    let blocks = messages
        .get(start..)
        .unwrap_or_default()
        .iter()
        .flat_map(|message| message.content.iter());
    let mut names: HashMap<&str, &str> = HashMap::new();
    let mut calls = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::ToolUse { id, name, .. } => {
                names.insert(id, name);
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error: false,
            } => {
                let Some(name) = names.get(tool_use_id.as_str()) else {
                    continue;
                };
                let text = match content {
                    ToolResultContent::Text(text) => text.clone(),
                    ToolResultContent::Json(value) => value.to_string(),
                };
                calls.push(((*name).to_string(), text));
            }
            _ => {}
        }
    }
    calls
}

/// The plan the last turn in `messages` wrote with `create_plan`, if any.
pub(crate) fn wrote_plan(messages: &[Message]) -> Option<String> {
    turn_calls(messages)
        .into_iter()
        .filter(|(name, _)| name == CREATE_PLAN_TOOL_NAME)
        .find_map(|(_, text)| plan_path_from_output(&text).map(str::to_string))
}

/// The pushes the last turn in `messages` made with the push tools.
pub(crate) fn pushes(messages: &[Message]) -> Vec<QueuedPush> {
    let pushes = turn_calls(messages)
        .into_iter()
        .filter(|(name, _)| is_push_tool(name))
        .filter_map(|(_, text)| QueuedPush::from_tool_output(&text))
        .collect();
    super::observe::distinct(pushes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How `create_plan` reports the plan it saved.
    const SAVED_PREFIX: &str = "Plan saved: ";

    const PUSHED: &str = r#"{"merge_queue":{"branch":"aura/a","head":"abc"}}"#;

    fn text(role: Role, text: &str) -> Message {
        Message {
            role,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    fn call(id: &str, name: &str, result: &str, is_error: bool) -> [Message; 2] {
        [
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: id.to_string(),
                    name: name.to_string(),
                    input: serde_json::json!({}),
                }],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: id.to_string(),
                    content: ToolResultContent::Text(result.to_string()),
                    is_error,
                }],
            },
        ]
    }

    #[test]
    fn only_the_last_turns_good_calls_count() {
        let mut messages = vec![text(Role::User, "first task")];
        messages.extend(call("1", "git_push", PUSHED, false));
        messages.push(text(Role::User, "second task"));
        messages.extend(call("2", "read_file", "x", false));
        messages.extend(call("3", "git_ship", "broken", true));
        let calls = turn_calls(&messages);
        assert_eq!(calls, [("read_file".to_string(), "x".to_string())]);
        assert!(pushes(&messages).is_empty());
    }

    #[test]
    fn pushes_count_only_from_push_tools() {
        let mut messages = vec![text(Role::User, "task")];
        messages.extend(call("1", "run_command", PUSHED, false));
        assert!(pushes(&messages).is_empty());
        messages.extend(call("2", "git_ship", PUSHED, false));
        messages.extend(call("3", "git_push", PUSHED, false));
        assert_eq!(pushes(&messages).len(), 1, "the repeat is dropped");
    }

    #[test]
    fn a_plan_counts_only_when_create_plan_wrote_it_this_turn() {
        let wrote = &format!("{SAVED_PREFIX}.aura/plans/a.plan.md");
        let mut messages = vec![text(Role::User, "task")];
        messages.extend(call("1", "read_file", wrote, false));
        assert_eq!(wrote_plan(&messages), None);
        messages.extend(call("2", CREATE_PLAN_TOOL_NAME, wrote, false));
        assert_eq!(
            wrote_plan(&messages).as_deref(),
            Some(".aura/plans/a.plan.md")
        );
    }
}
