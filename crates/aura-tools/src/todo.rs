//! `todo_write`: the model's working plan for a long task.
//!
//! The tool itself is stateless — it validates the arguments and echoes
//! the items back. The agent loop owns the current list (it applies
//! each call with [`apply_todo_write`]) so the plan survives context
//! compaction and drives "keep going while items are open".

use async_trait::async_trait;
use aura_core_types::{ToolDefinition, ToolResult};
use serde::{Deserialize, Serialize};

use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};

/// Tool name used for dispatch and for spotting calls in a transcript.
pub const TODO_WRITE_TOOL_NAME: &str = "todo_write";

/// Lifecycle of one todo item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    /// Not started.
    Pending,
    /// Being worked on now.
    InProgress,
    /// Done.
    Completed,
    /// Dropped; no longer needed.
    Cancelled,
}

impl TodoStatus {
    /// Whether the item still needs work.
    #[must_use]
    pub fn is_open(self) -> bool {
        matches!(self, Self::Pending | Self::InProgress)
    }

    fn marker(self) -> &'static str {
        match self {
            Self::Pending => "[ ]",
            Self::InProgress => "[~]",
            Self::Completed => "[x]",
            Self::Cancelled => "[-]",
        }
    }
}

/// One item on the todo list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    /// Stable identifier the model uses to update the item.
    pub id: String,
    /// What needs doing.
    pub content: String,
    /// Current status.
    pub status: TodoStatus,
}

#[derive(Deserialize)]
struct TodoWriteArgs {
    todos: Vec<TodoPatch>,
    #[serde(default)]
    merge: bool,
}

#[derive(Deserialize)]
struct TodoPatch {
    id: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    status: Option<TodoStatus>,
}

/// Apply one `todo_write` call to `current` and return the new list.
///
/// With `merge: false` the call replaces the list, so every item needs
/// content. With `merge: true` items are matched by id: matching items
/// get the supplied fields, unknown ids are appended.
///
/// # Errors
/// Returns a message for the model when the arguments are malformed.
pub fn apply_todo_write(
    current: &[TodoItem],
    args: &serde_json::Value,
) -> Result<Vec<TodoItem>, String> {
    let parsed: TodoWriteArgs = serde_json::from_value(args.clone())
        .map_err(|e| format!("invalid todo_write arguments: {e}"))?;

    let mut next: Vec<TodoItem> = if parsed.merge {
        current.to_vec()
    } else {
        Vec::new()
    };
    for patch in parsed.todos {
        if patch.id.trim().is_empty() {
            return Err("every todo needs a non-empty id".into());
        }
        if let Some(existing) = next.iter_mut().find(|item| item.id == patch.id) {
            if let Some(content) = patch.content {
                existing.content = content;
            }
            if let Some(status) = patch.status {
                existing.status = status;
            }
            continue;
        }
        let Some(content) = patch.content.filter(|c| !c.trim().is_empty()) else {
            return Err(format!("new todo '{}' needs content", patch.id));
        };
        next.push(TodoItem {
            id: patch.id,
            content,
            status: patch.status.unwrap_or(TodoStatus::Pending),
        });
    }
    Ok(next)
}

/// Check a `todo_write` call without knowing the current list. A merge
/// may update an existing id without content, so only a replacement
/// can be checked for content here.
///
/// # Errors
/// Returns a message for the model when the arguments are malformed.
pub fn validate_todo_write(args: &serde_json::Value) -> Result<(), String> {
    let merge = args
        .get("merge")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if merge {
        let parsed: TodoWriteArgs = serde_json::from_value(args.clone())
            .map_err(|e| format!("invalid todo_write arguments: {e}"))?;
        if parsed.todos.iter().any(|patch| patch.id.trim().is_empty()) {
            return Err("every todo needs a non-empty id".into());
        }
        Ok(())
    } else {
        apply_todo_write(&[], args).map(|_| ())
    }
}

/// Whether any item still needs work.
#[must_use]
pub fn has_open_todos(todos: &[TodoItem]) -> bool {
    todos.iter().any(|item| item.status.is_open())
}

/// Plain-text checklist, one item per line.
#[must_use]
pub fn render_todos(todos: &[TodoItem]) -> String {
    let mut out = String::new();
    for item in todos {
        out.push_str(item.status.marker());
        out.push(' ');
        out.push_str(&item.content);
        out.push_str(" (id: ");
        out.push_str(&item.id);
        out.push_str(")\n");
    }
    out
}

/// The `todo_write` tool.
pub struct TodoWriteTool;

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        TODO_WRITE_TOOL_NAME
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: TODO_WRITE_TOOL_NAME.into(),
            description: "Create or update your todo list for the current task. Use it for \
                any task with three or more steps: write the plan up front, keep exactly one \
                item in_progress, and mark items completed as soon as they are done. With \
                merge=true only the listed ids change; otherwise the list is replaced. The \
                user sees this list, and you are expected to keep working until every item \
                is completed or cancelled."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": {"type": "string", "description": "Stable id, e.g. \"1\" or \"add-tests\""},
                                "content": {"type": "string", "description": "What needs doing. Required for new items."},
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed", "cancelled"]
                                }
                            },
                            "required": ["id"]
                        }
                    },
                    "merge": {
                        "type": "boolean",
                        "description": "true: update listed ids and append new ones. false (default): replace the whole list."
                    }
                },
                "required": ["todos"]
            }),
            cache_control: None,
            eager_input_streaming: None,
        }
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        validate_todo_write(&args).map_err(ToolError::InvalidArguments)?;
        Ok(ToolResult::success(
            TODO_WRITE_TOOL_NAME,
            "Todo list updated. Continue with the in_progress item.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item(id: &str, content: &str, status: TodoStatus) -> TodoItem {
        TodoItem {
            id: id.into(),
            content: content.into(),
            status,
        }
    }

    #[test]
    fn replace_sets_whole_list() {
        let current = vec![item("old", "old", TodoStatus::Pending)];
        let next = apply_todo_write(
            &current,
            &json!({"todos": [{"id": "1", "content": "a"}, {"id": "2", "content": "b", "status": "in_progress"}]}),
        )
        .unwrap();
        assert_eq!(
            next,
            vec![
                item("1", "a", TodoStatus::Pending),
                item("2", "b", TodoStatus::InProgress)
            ]
        );
    }

    #[test]
    fn merge_updates_by_id_and_appends() {
        let current = vec![
            item("1", "a", TodoStatus::InProgress),
            item("2", "b", TodoStatus::Pending),
        ];
        let next = apply_todo_write(
            &current,
            &json!({"merge": true, "todos": [{"id": "1", "status": "completed"}, {"id": "3", "content": "c"}]}),
        )
        .unwrap();
        assert_eq!(
            next,
            vec![
                item("1", "a", TodoStatus::Completed),
                item("2", "b", TodoStatus::Pending),
                item("3", "c", TodoStatus::Pending)
            ]
        );
        assert!(has_open_todos(&next));
    }

    #[test]
    fn new_item_without_content_is_rejected() {
        let err = apply_todo_write(&[], &json!({"todos": [{"id": "1"}]})).unwrap_err();
        assert!(err.contains("needs content"));
    }

    #[test]
    fn render_marks_status() {
        let text = render_todos(&[
            item("1", "a", TodoStatus::Completed),
            item("2", "b", TodoStatus::InProgress),
        ]);
        assert_eq!(text, "[x] a (id: 1)\n[~] b (id: 2)\n");
    }
}
