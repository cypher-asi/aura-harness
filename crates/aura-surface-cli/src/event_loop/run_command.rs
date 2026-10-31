//! Run a shell command typed in the TUI as a `run_command` tool call.
//!
//! The command becomes a `ToolProposal` transaction (source `user`) that
//! goes through the agent's inbox and `Kernel::process_dequeued`, so it
//! gets the same policy check, sandboxed execution and record entries
//! (proposal, policy decision, execution) as a tool call proposed by the
//! model. It runs in a pseudo-terminal as wide
//! as the chat, so the TUI can show its output as a terminal would. The
//! call and its result (without ANSI styling) are then appended to the
//! conversation as a tool-use / tool-result pair.

use super::{drain_stale_inbox, LoopState};
use aura_agent_kernel::{Kernel, KernelError, ProcessResult};
use aura_core_types::{ToolProposal, Transaction};
use aura_model_reasoner::{ContentBlock, Message, Role, ToolResultContent};
use aura_terminal::{ansi::strip_ansi, UiCommand};
use serde_json::{json, Value};
use tracing::{error, info};

/// Tool used for commands typed by the user.
pub(crate) const RUN_COMMAND_TOOL: &str = "run_command";

/// `ToolProposal::source` for proposals that come from the user.
pub(crate) const USER_SOURCE: &str = "user";

/// `tool_use_id` prefix of proposals that come from the user.
const USER_TOOL_USE_PREFIX: &str = "user_";

/// Whether `tool_use_id` belongs to a command the user typed.
pub(crate) fn is_user_tool_use(tool_use_id: &str) -> bool {
    tool_use_id.starts_with(USER_TOOL_USE_PREFIX)
}

/// Characters that need a shell to interpret.
const SHELL_SYNTAX: &[char] = &[
    '|', '&', ';', '<', '>', '(', ')', '$', '`', '\\', '"', '\'', '*', '?', '[', ']', '{', '}',
    '~', '#', '=', '%', '\n',
];

/// `run_command` arguments for a typed command line, run in a terminal
/// `tty_cols` wide when given.
///
/// Plain commands use the direct `program` + `args` form; anything with
/// shell syntax uses the opt-in `shell_script` form.
pub(crate) fn command_args(command: &str, tty_cols: Option<u16>) -> Value {
    let mut args = if command.contains(SHELL_SYNTAX) {
        json!({ "shell_script": command, "allow_shell": true })
    } else {
        let mut words = command.split_whitespace();
        let program = words.next().unwrap_or_default();
        let args: Vec<&str> = words.collect();
        json!({ "program": program, "args": args })
    };
    if let Some(cols) = tty_cols {
        args["tty_cols"] = json!(cols);
    }
    args
}

/// Build the tool proposal for a typed command.
pub(crate) fn user_command_proposal(command: &str, tty_cols: Option<u16>) -> ToolProposal {
    let tool_use_id = format!("{USER_TOOL_USE_PREFIX}{}", uuid::Uuid::new_v4().simple());
    ToolProposal::new(
        tool_use_id,
        RUN_COMMAND_TOOL,
        command_args(command, tty_cols),
    )
    .with_source(USER_SOURCE)
}

/// Enqueue `proposal` in the agent's inbox and process it through the kernel.
pub(crate) async fn process_user_proposal(
    kernel: &Kernel,
    proposal: &ToolProposal,
) -> Result<(Transaction, ProcessResult), KernelError> {
    let store = kernel.store();
    let agent_id = kernel.agent_id;
    let tx = Transaction::tool_proposal(agent_id, proposal)
        .map_err(|e| KernelError::Serialization(e.to_string()))?;
    store
        .enqueue_tx(&tx)
        .map_err(|e| KernelError::Store(format!("enqueue_tx: {e}")))?;
    let (token, dequeued) = store
        .dequeue_tx(agent_id)
        .map_err(|e| KernelError::Store(format!("dequeue_tx: {e}")))?
        .ok_or_else(|| KernelError::Internal("command was enqueued but not found".into()))?;
    if dequeued.hash != tx.hash {
        return Err(KernelError::Internal(
            "inbox returned a different transaction than the command".into(),
        ));
    }
    let result = kernel.process_dequeued(dequeued.clone(), token).await?;
    Ok((dequeued, result))
}

/// What the terminal shows for a command's tool output: its output text
/// and, when it failed, its exit code.
///
/// Command results arrive as the `{ ok, exit_code, stdout, stderr }`
/// envelope. Anything else is an error from before the command ran
/// (policy, spawn), shown the way a shell reports its own errors.
pub(crate) fn terminal_output(content: &str, is_error: bool) -> (String, Option<i32>) {
    let Some(envelope) = serde_json::from_str::<Value>(content)
        .ok()
        .filter(|v| v.get("stdout").is_some())
    else {
        let text = if is_error {
            format!("aura: {content}\n")
        } else {
            content.to_string()
        };
        return (text, None);
    };
    let field = |key: &str| envelope.get(key).and_then(Value::as_str).unwrap_or("");
    let mut output = field("stdout").to_string();
    output.push_str(field("stderr"));
    let ok = envelope
        .get("ok")
        .and_then(Value::as_bool)
        .unwrap_or(!is_error);
    let exit_code = envelope
        .get("exit_code")
        .and_then(Value::as_i64)
        .and_then(|code| i32::try_from(code).ok())
        .filter(|_| !ok);
    (output, exit_code)
}

/// `content` with ANSI styling removed from the envelope's output fields,
/// for the model's copy of the result.
pub(crate) fn model_content(content: &str) -> String {
    let Ok(mut envelope) = serde_json::from_str::<Value>(content) else {
        return strip_ansi(content);
    };
    let Some(fields) = envelope.as_object_mut() else {
        return strip_ansi(content);
    };
    for key in ["stdout", "stderr"] {
        if let Some(Value::String(text)) = fields.get_mut(key) {
            *text = strip_ansi(text);
        }
    }
    envelope.to_string()
}

pub(super) async fn handle_run_command(state: &mut LoopState<'_>, command: &str, cols: u16) {
    info!(command = %command, cols, "Running user command");

    drain_stale_inbox(state).await;

    let proposal = user_command_proposal(command, Some(cols));
    let kernel = state.session.kernel.clone();
    let (_, result) = match process_user_proposal(&kernel, &proposal).await {
        Ok(processed) => processed,
        Err(e) => {
            error!(error = %e, "Failed to process user command via kernel");
            let _ = state
                .commands
                .send(UiCommand::CommandFinished {
                    output: format!("aura: kernel error: {e}\n"),
                    exit_code: None,
                })
                .await;
            let _ = state.commands.send(UiCommand::Complete).await;
            return;
        }
    };

    let (content, is_error) = result.tool_output.map_or_else(
        || ("No output from kernel".to_string(), true),
        |o| (o.content, o.is_error),
    );

    let (output, exit_code) = terminal_output(&content, is_error);
    let _ = state
        .commands
        .send(UiCommand::CommandFinished { output, exit_code })
        .await;

    state.session.messages.push(Message::new(
        Role::Assistant,
        vec![ContentBlock::tool_use(
            proposal.tool_use_id.clone(),
            proposal.tool,
            proposal.args,
        )],
    ));
    state.session.messages.push(Message::tool_results(vec![(
        proposal.tool_use_id,
        ToolResultContent::text(model_content(&content)),
        is_error,
    )]));

    let _ = state.commands.send(UiCommand::Complete).await;
}

#[cfg(test)]
mod tests;
