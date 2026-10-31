//! Run a shell command typed in the TUI as a `run_command` tool call.
//!
//! The command becomes a `ToolProposal` transaction (source `user`) that
//! goes through the agent's inbox and `Kernel::process_dequeued`, so it
//! gets the same policy check, sandboxed execution and record entry as a
//! tool call proposed by the model. The call and its result are then
//! appended to the conversation as a tool-use / tool-result pair.

use super::{drain_stale_inbox, send_record_to_ui, LoopState};
use aura_agent_kernel::{Kernel, KernelError, ProcessResult};
use aura_core_types::{ToolProposal, Transaction};
use aura_model_reasoner::{ContentBlock, Message, Role, ToolResultContent};
use aura_terminal::{events::ToolData, UiCommand};
use serde_json::{json, Value};
use tracing::{error, info};

/// Tool used for commands typed by the user.
pub(crate) const RUN_COMMAND_TOOL: &str = "run_command";

/// `ToolProposal::source` for proposals that come from the user.
pub(crate) const USER_SOURCE: &str = "user";

/// Characters that need a shell to interpret.
const SHELL_SYNTAX: &[char] = &[
    '|', '&', ';', '<', '>', '(', ')', '$', '`', '\\', '"', '\'', '*', '?', '[', ']', '{', '}',
    '~', '#', '=', '%', '\n',
];

/// `run_command` arguments for a typed command line.
///
/// Plain commands use the direct `program` + `args` form; anything with
/// shell syntax uses the opt-in `shell_script` form.
pub(crate) fn command_args(command: &str) -> Value {
    if command.contains(SHELL_SYNTAX) {
        return json!({ "shell_script": command, "allow_shell": true });
    }
    let mut words = command.split_whitespace();
    let program = words.next().unwrap_or_default();
    let args: Vec<&str> = words.collect();
    json!({ "program": program, "args": args })
}

/// Build the tool proposal for a typed command.
pub(crate) fn user_command_proposal(command: &str) -> ToolProposal {
    let tool_use_id = format!("user_{}", uuid::Uuid::new_v4().simple());
    ToolProposal::new(tool_use_id, RUN_COMMAND_TOOL, command_args(command)).with_source(USER_SOURCE)
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

pub(super) async fn handle_run_command(state: &mut LoopState<'_>, command: &str) {
    info!(command = %command, "Running user command");

    drain_stale_inbox(state).await;

    let proposal = user_command_proposal(command);
    let _ = state
        .commands
        .send(UiCommand::ShowTool(ToolData {
            id: proposal.tool_use_id.clone(),
            name: proposal.tool.clone(),
            args: proposal.args.to_string(),
        }))
        .await;

    let kernel = state.session.kernel.clone();
    let (tx, result) = match process_user_proposal(&kernel, &proposal).await {
        Ok(processed) => processed,
        Err(e) => {
            error!(error = %e, "Failed to process user command via kernel");
            let _ = state
                .commands
                .send(UiCommand::CompleteTool {
                    id: proposal.tool_use_id.clone(),
                    result: String::new(),
                    success: false,
                })
                .await;
            let _ = state
                .commands
                .send(UiCommand::ShowError(format!("Kernel error: {e}")))
                .await;
            let _ = state.commands.send(UiCommand::Complete).await;
            return;
        }
    };

    send_record_to_ui(state.commands, result.entry.seq, &tx, &result.entry).await;

    let (content, is_error) = result.tool_output.map_or_else(
        || ("No output from kernel".to_string(), true),
        |o| (o.content, o.is_error),
    );

    let _ = state
        .commands
        .send(UiCommand::CompleteTool {
            id: proposal.tool_use_id.clone(),
            result: content.clone(),
            success: !is_error,
        })
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
        ToolResultContent::text(content),
        is_error,
    )]));

    let _ = state.commands.send(UiCommand::Complete).await;
}

#[cfg(test)]
mod tests;
