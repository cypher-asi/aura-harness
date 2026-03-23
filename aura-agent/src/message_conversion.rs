//! Chat message conversion and project-aware tool routing.
//!
//! Converts presentation-layer chat messages into the
//! [`aura_reasoner::Message`] format, and provides a forwarding executor
//! that routes tool calls to the correct project.

use std::collections::HashSet;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;

use aura_reasoner::{ContentBlock, ImageSource, Message, Role, ToolResultContent};

use crate::types::{AgentToolExecutor, ToolCallInfo, ToolCallResult};

// ---------------------------------------------------------------------------
// Chat message types (presentation-layer format)
// ---------------------------------------------------------------------------

/// Chat role in the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
    System,
}

/// A chat message from the presentation layer.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    pub content_blocks: Option<Vec<ChatContentBlock>>,
}

/// Content block in a chat message.
#[derive(Debug, Clone)]
pub enum ChatContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: Option<bool>,
    },
    Image {
        media_type: String,
        data: String,
    },
}

/// Attachment to a chat message.
#[derive(Debug, Clone)]
pub struct ChatAttachment {
    /// Type: `"image"` or `"text"`.
    pub type_: String,
    pub media_type: String,
    /// Base64-encoded data.
    pub data: String,
    pub name: Option<String>,
}

// ---------------------------------------------------------------------------
// Message conversion
// ---------------------------------------------------------------------------

fn convert_content_blocks(blocks: &[ChatContentBlock], role: Role) -> Vec<Message> {
    let mut primary_blocks: Vec<ContentBlock> = Vec::new();
    let mut tool_result_blocks: Vec<ContentBlock> = Vec::new();

    for b in blocks {
        match b {
            ChatContentBlock::Text { text } => {
                primary_blocks.push(ContentBlock::Text { text: text.clone() });
            }
            ChatContentBlock::ToolUse { id, name, input } => {
                primary_blocks.push(ContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                });
            }
            ChatContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                let block = ContentBlock::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: ToolResultContent::Text(content.clone()),
                    is_error: is_error.unwrap_or(false),
                };
                if role == Role::Assistant {
                    tool_result_blocks.push(block);
                } else {
                    primary_blocks.push(block);
                }
            }
            ChatContentBlock::Image { media_type, data } => {
                primary_blocks.push(ContentBlock::Image {
                    source: ImageSource {
                        source_type: "base64".to_string(),
                        media_type: media_type.clone(),
                        data: data.clone(),
                    },
                });
            }
        }
    }

    let mut result = vec![Message::new(role, primary_blocks)];
    if !tool_result_blocks.is_empty() {
        result.push(Message::new(Role::User, tool_result_blocks));
    }
    result
}

/// Convert presentation-layer chat messages to [`aura_reasoner::Message`]s.
///
/// System messages are filtered out. Assistant messages with `ToolResult`
/// blocks are split so that tool results appear in a separate user message
/// (matching the API requirement).
pub fn convert_messages_to_rich(messages: &[ChatMessage]) -> Vec<Message> {
    messages
        .iter()
        .filter(|m| m.role == ChatRole::User || m.role == ChatRole::Assistant)
        .flat_map(|m| {
            let role = match m.role {
                ChatRole::User => Role::User,
                ChatRole::Assistant => Role::Assistant,
                ChatRole::System => Role::User,
            };
            if let Some(blocks) = &m.content_blocks {
                convert_content_blocks(blocks, role)
            } else {
                vec![Message::new(
                    role,
                    vec![ContentBlock::Text {
                        text: m.content.clone(),
                    }],
                )]
            }
        })
        .collect()
}

/// Build [`ChatContentBlock`]s from text content and attachments.
///
/// Returns `None` if there are no attachments.
pub fn build_attachment_blocks(
    content: &str,
    attachments: &[ChatAttachment],
) -> Option<Vec<ChatContentBlock>> {
    if attachments.is_empty() {
        return None;
    }
    let mut blocks: Vec<ChatContentBlock> = Vec::new();
    if !content.trim().is_empty() {
        blocks.push(ChatContentBlock::Text {
            text: content.to_string(),
        });
    }
    for att in attachments {
        if att.type_ == "image" {
            blocks.push(ChatContentBlock::Image {
                media_type: att.media_type.clone(),
                data: att.data.clone(),
            });
        } else if att.type_ == "text" {
            let text = match B64.decode(&att.data) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(e) => {
                    tracing::warn!(
                        name = att.name.as_deref().unwrap_or("<unnamed>"),
                        error = %e,
                        "Skipping text attachment with invalid base64",
                    );
                    continue;
                }
            };
            let header = att
                .name
                .as_deref()
                .map(|n| format!("[File: {}]\n\n", n))
                .unwrap_or_default();
            blocks.push(ChatContentBlock::Text {
                text: format!("{}{}", header, text),
            });
        }
    }
    if blocks.is_empty() {
        None
    } else {
        Some(blocks)
    }
}

// ---------------------------------------------------------------------------
// Project resolution
// ---------------------------------------------------------------------------

/// Resolve a project identifier from a tool call's input.
///
/// Implementations define how a tool call maps to a project (single-project
/// mode vs multi-project routing).
pub trait ProjectResolver: Send + Sync {
    fn resolve(&self, input: &serde_json::Value) -> Result<String, &'static str>;

    /// Whether `create_task` calls should be serialized (to preserve ordering).
    fn sequential_create_task(&self) -> bool;
}

/// Routes all tool calls to a single project.
pub struct SingleProjectResolver {
    pub project_id: String,
}

impl ProjectResolver for SingleProjectResolver {
    fn resolve(&self, _input: &serde_json::Value) -> Result<String, &'static str> {
        Ok(self.project_id.clone())
    }

    fn sequential_create_task(&self) -> bool {
        true
    }
}

/// Routes tool calls to one of several allowed projects based on a
/// `project_id` field in the tool input.
pub struct MultiProjectResolver {
    pub allowed_project_ids: HashSet<String>,
}

impl ProjectResolver for MultiProjectResolver {
    fn resolve(&self, input: &serde_json::Value) -> Result<String, &'static str> {
        let pid_str = input
            .get("project_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if self.allowed_project_ids.contains(pid_str) {
            Ok(pid_str.to_string())
        } else {
            Err("Missing or invalid project_id. You must specify a valid project_id from the available projects.")
        }
    }

    fn sequential_create_task(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Forwarding executor
// ---------------------------------------------------------------------------

/// Callback invoked after each tool call result for event forwarding.
pub trait ToolResultCallback: Send + Sync {
    fn on_result(&self, tc: &ToolCallInfo, result: &ToolCallResult);
}

/// Tool executor that wraps an [`AgentToolExecutor`] with project routing
/// and optional result forwarding.
pub struct RoutingToolExecutor<R: ProjectResolver> {
    pub inner: std::sync::Arc<dyn AgentToolExecutor>,
    pub resolver: R,
}

#[async_trait]
impl<R: ProjectResolver + 'static> AgentToolExecutor for RoutingToolExecutor<R> {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        // Validate all project IDs first
        let mut valid_calls: Vec<(usize, ToolCallInfo)> = Vec::new();
        let mut error_results: Vec<(usize, ToolCallResult)> = Vec::new();

        for (i, tc) in tool_calls.iter().enumerate() {
            match self.resolver.resolve(&tc.input) {
                Ok(_pid) => valid_calls.push((i, tc.clone())),
                Err(msg) => error_results.push((
                    i,
                    ToolCallResult::error(tc.id.clone(), msg),
                )),
            }
        }

        let valid_tool_calls: Vec<ToolCallInfo> =
            valid_calls.iter().map(|(_, tc)| tc.clone()).collect();
        let inner_results = if valid_tool_calls.is_empty() {
            Vec::new()
        } else {
            self.inner.execute(&valid_tool_calls).await
        };

        // Merge results back in order
        let mut result_map: Vec<(usize, ToolCallResult)> = Vec::with_capacity(tool_calls.len());
        let mut inner_iter = inner_results.into_iter();
        for (i, _) in &valid_calls {
            if let Some(r) = inner_iter.next() {
                result_map.push((*i, r));
            }
        }
        result_map.extend(error_results);
        result_map.sort_by_key(|(i, _)| *i);
        result_map.into_iter().map(|(_, r)| r).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_message(role: ChatRole, content: &str) -> ChatMessage {
        ChatMessage {
            role,
            content: content.into(),
            content_blocks: None,
        }
    }

    #[test]
    fn convert_empty_messages() {
        let result = convert_messages_to_rich(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn convert_text_only_messages() {
        let messages = vec![
            make_message(ChatRole::User, "Hello"),
            make_message(ChatRole::Assistant, "Hi there"),
        ];
        let rich = convert_messages_to_rich(&messages);
        assert_eq!(rich.len(), 2);
        assert_eq!(rich[0].role, Role::User);
        assert_eq!(rich[1].role, Role::Assistant);
        assert_eq!(rich[0].text_content(), "Hello");
    }

    #[test]
    fn convert_system_message_filtered() {
        let messages = vec![make_message(ChatRole::System, "system msg")];
        let rich = convert_messages_to_rich(&messages);
        assert!(rich.is_empty(), "System messages should be filtered out");
    }

    #[test]
    fn convert_messages_with_content_blocks() {
        let mut msg = make_message(ChatRole::User, "");
        msg.content_blocks = Some(vec![
            ChatContentBlock::Text {
                text: "check this".into(),
            },
            ChatContentBlock::ToolUse {
                id: "t1".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "a.rs"}),
            },
            ChatContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                content: "file contents".into(),
                is_error: None,
            },
            ChatContentBlock::Image {
                media_type: "image/png".into(),
                data: "base64data".into(),
            },
        ]);

        let rich = convert_messages_to_rich(&[msg]);
        assert_eq!(rich.len(), 1);
        assert_eq!(rich[0].content.len(), 4);
    }

    #[test]
    fn convert_splits_tool_results_from_assistant() {
        let mut msg = make_message(ChatRole::Assistant, "");
        msg.content_blocks = Some(vec![
            ChatContentBlock::Text {
                text: "I'll read the file".into(),
            },
            ChatContentBlock::ToolUse {
                id: "t1".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "a.rs"}),
            },
            ChatContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                content: "file contents".into(),
                is_error: None,
            },
        ]);

        let rich = convert_messages_to_rich(&[msg]);
        assert_eq!(rich.len(), 2);
        assert_eq!(rich[0].role, Role::Assistant);
        assert_eq!(rich[1].role, Role::User);
        assert_eq!(rich[0].content.len(), 2);
        assert_eq!(rich[1].content.len(), 1);
    }

    #[test]
    fn convert_filters_system_keeps_user_and_assistant() {
        let messages = vec![
            make_message(ChatRole::System, "sys"),
            make_message(ChatRole::User, "u1"),
            make_message(ChatRole::Assistant, "a1"),
            make_message(ChatRole::User, "u2"),
        ];
        let rich = convert_messages_to_rich(&messages);
        assert_eq!(rich.len(), 3);
        assert_eq!(rich[0].role, Role::User);
        assert_eq!(rich[1].role, Role::Assistant);
        assert_eq!(rich[2].role, Role::User);
    }

    #[test]
    fn single_project_resolver_always_returns_same_id() {
        let resolver = SingleProjectResolver {
            project_id: "proj-1".into(),
        };
        assert_eq!(
            resolver.resolve(&serde_json::json!({})).unwrap(),
            "proj-1",
        );
        assert!(resolver.sequential_create_task());
    }

    #[test]
    fn multi_project_resolver_validates_id() {
        let resolver = MultiProjectResolver {
            allowed_project_ids: HashSet::from(["proj-1".to_string()]),
        };
        assert!(resolver
            .resolve(&serde_json::json!({"project_id": "proj-1"}))
            .is_ok());
        assert!(resolver
            .resolve(&serde_json::json!({"project_id": "proj-2"}))
            .is_err());
        assert!(resolver.resolve(&serde_json::json!({})).is_err());
        assert!(!resolver.sequential_create_task());
    }
}
