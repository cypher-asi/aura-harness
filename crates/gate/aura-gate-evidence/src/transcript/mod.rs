//! Reading a turn's messages.
//!
//! - [`RUN_COMMAND`] and [`FILE_TOOLS`]: the tools that run commands and
//!   change files
//! - `command`: the command lines of the `run_command` calls
//!   ([`command_lines`])
//! - `output`: what a command printed, read once from its result
//! - [`reply`]: the final reply, and what it says of the work
//! - [`said`]: Gate's own messages among them

use aura_model::{ContentBlock, Message, ToolResultContent};
use serde_json::Value;

pub(crate) mod command;
mod output;
pub mod reply;
pub mod said;

pub use command::command_lines;
pub(crate) use output::{shows_failures, Output};

/// Name of the tool that runs commands.
pub const RUN_COMMAND: &str = "run_command";

/// Tools that change a file.
pub const FILE_TOOLS: [&str; 3] = ["write_file", "edit_file", "delete_file"];

/// A tool call or result in a turn's messages.
#[derive(Debug)]
pub(crate) enum Tool<'a> {
    /// A `run_command` call: its id, its place among the tool calls (0
    /// for the first), the command line it runs, and whether it went to
    /// the background
    Command {
        id: &'a str,
        at: usize,
        line: String,
        background: bool,
    },
    /// A call of another tool: its place among the tool calls, its name
    /// and its input
    Call {
        at: usize,
        name: &'a str,
        input: &'a Value,
    },
    /// A tool's result: its call's id, what it holds, and whether the
    /// call failed
    Result {
        id: &'a str,
        content: &'a ToolResultContent,
        is_error: bool,
    },
}

/// Reads a turn's tool calls and results in order, numbering the calls.
#[derive(Debug, Default)]
pub(crate) struct Tools {
    /// Tool calls read so far
    calls: usize,
}

impl Tools {
    /// The tool call or result `block` holds; `None` for other content.
    pub(crate) fn read<'a>(&mut self, block: &'a ContentBlock) -> Option<Tool<'a>> {
        match block {
            ContentBlock::ToolUse { id, name, input } => {
                let at = self.calls;
                self.calls += 1;
                Some(if name == RUN_COMMAND {
                    Tool::Command {
                        id,
                        at,
                        line: command::command_line(input),
                        background: input["background"].as_bool() == Some(true),
                    }
                } else {
                    Tool::Call { at, name, input }
                })
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Some(Tool::Result {
                id: tool_use_id,
                content,
                is_error: *is_error,
            }),
            _ => None,
        }
    }
}

/// The tool calls and results in `messages`, in order.
pub(crate) fn tools(messages: &[Message]) -> impl Iterator<Item = Tool<'_>> {
    let mut tools = Tools::default();
    messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(move |block| tools.read(block))
}
