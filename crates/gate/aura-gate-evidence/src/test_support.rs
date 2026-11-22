//! Builders of a turn's messages for tests: its tool calls, their
//! results and its reply.

use aura_model::{ContentBlock, Message, Role, ToolResultContent};
use serde_json::{json, Value};

use crate::transcript::RUN_COMMAND;

/// The model calling `tool` with `input`, as call `id`.
#[must_use]
pub fn call(id: &str, tool: &str, input: Value) -> Message {
    Message::new(
        Role::Assistant,
        vec![ContentBlock::tool_use(id, tool, input)],
    )
}

/// The model calling `run_command` with `input`, as call `id`.
#[must_use]
pub fn run(id: &str, input: Value) -> Message {
    call(id, RUN_COMMAND, input)
}

/// The result of call `id`: `content`, as text, and whether the call
/// failed.
#[must_use]
pub fn result(id: &str, content: &str, is_error: bool) -> Message {
    Message::tool_results(vec![(
        id.to_string(),
        ToolResultContent::text(content),
        is_error,
    )])
}

/// The model running `command` as call `id`, and the call's result:
/// `output`, and whether the call failed.
#[must_use]
pub fn ran(id: &str, command: &str, output: &str, is_error: bool) -> [Message; 2] {
    [
        run(id, json!({ "command": command })),
        result(id, output, is_error),
    ]
}

/// The model's reply `text`.
#[must_use]
pub fn reply(text: impl Into<String>) -> Message {
    Message::assistant(text)
}
