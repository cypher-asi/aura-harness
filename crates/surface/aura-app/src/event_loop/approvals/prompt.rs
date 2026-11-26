//! The kernel's side of [`TerminalApprovals`]: deciding whether a call
//! needs the user at all, and putting the question to them when it does.

use super::super::run_command::{is_user_tool_use, RUN_COMMAND_TOOL};
use super::rules::{command_words, stays_in_folder};
use super::{lock, ApprovalRule, TerminalApprovals, Waiting};
use async_trait::async_trait;
use aura_agent_kernel::{
    PendingToolPrompt, ToolApprovalError, ToolApprovalPrompter, ToolApprovalRemember,
    ToolApprovalResponse,
};
use aura_core_types::text::truncate_with_ellipsis;
use aura_core_types::ToolState;
use aura_ui::{ApprovalLevel, UiCommand};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;
use tokio::sync::oneshot;
use tracing::info;

/// How long a prompt may take to reach the UI. The command channel only
/// stays full while the UI is gone.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(2);

/// Longest description of a call shown in the prompt.
const MAX_DESCRIPTION_CHARS: usize = 400;

const fn once(decision: ToolState) -> ToolApprovalResponse {
    ToolApprovalResponse {
        decision,
        remember: ToolApprovalRemember::Once,
    }
}

/// Whether the call is a command the user typed in the chat, which is
/// already their decision. The kernel's request id is
/// `<agent hex>:<tool use id>`.
fn is_typed(prompt: &PendingToolPrompt) -> bool {
    prompt
        .request_id
        .split_once(':')
        .is_some_and(|(_, tool_use_id)| is_user_tool_use(tool_use_id))
}

/// The "always allow" offer of the prompt, or empty when there is none.
fn always_offer(prompt: &PendingToolPrompt, folder: Option<&Path>, level: ApprovalLevel) -> String {
    match (
        ApprovalRule::for_call(&prompt.tool_name, &prompt.args),
        folder,
    ) {
        (Some(rule), Some(_)) if level != ApprovalLevel::Strict => {
            format!("{} in this project", rule.label())
        }
        _ => String::new(),
    }
}

impl TerminalApprovals {
    /// Whether the call may run without asking: at `yolo` a command that
    /// stays in the project folder, below `strict` a call a project rule
    /// covers.
    fn allowed_without_prompt(
        &self,
        prompt: &PendingToolPrompt,
        folder: Option<&Path>,
        level: ApprovalLevel,
    ) -> bool {
        if level == ApprovalLevel::Yolo
            && prompt.tool_name == RUN_COMMAND_TOOL
            && folder.is_some_and(|folder| stays_in_folder(&prompt.args, folder))
        {
            info!("Allowed a command that stays in the project folder (yolo)");
            return true;
        }
        let ruled = level != ApprovalLevel::Strict
            && folder.is_some_and(|folder| {
                lock(&self.rules).allows(folder, &prompt.tool_name, &prompt.args)
            });
        if ruled {
            info!(tool = %prompt.tool_name, "Allowed by a project approval rule");
        }
        ruled
    }

    /// Note the call as waiting for its answer, returning where the answer
    /// arrives.
    fn start_waiting(&self, prompt: &PendingToolPrompt) -> oneshot::Receiver<ToolState> {
        let (reply_tx, reply_rx) = oneshot::channel();
        lock(&self.pending).insert(
            prompt.request_id.clone(),
            Waiting {
                agent_id: prompt.agent_id,
                tool: prompt.tool_name.clone(),
                args: prompt.args.clone(),
                reply: reply_tx,
            },
        );
        reply_rx
    }

    /// Show the prompt in the agent's chat. When it cannot be delivered
    /// the call stops waiting.
    async fn deliver_prompt(
        &self,
        prompt: &PendingToolPrompt,
        always: String,
    ) -> Result<(), ToolApprovalError> {
        let request = UiCommand::ForAgent {
            agent_id: prompt.agent_id.to_hex(),
            command: Box::new(UiCommand::RequestApproval {
                id: prompt.request_id.clone(),
                tool: prompt.tool_name.clone(),
                description: describe(&prompt.tool_name, &prompt.args),
                always,
            }),
        };
        let delivered = tokio::time::timeout(DELIVERY_TIMEOUT, self.commands.send(request)).await;
        if !matches!(delivered, Ok(Ok(()))) {
            lock(&self.pending).remove(&prompt.request_id);
            return Err(ToolApprovalError::DeliveryFailed);
        }
        Ok(())
    }
}

#[async_trait]
impl ToolApprovalPrompter for TerminalApprovals {
    async fn prompt(
        &self,
        prompt: PendingToolPrompt,
    ) -> Result<ToolApprovalResponse, ToolApprovalError> {
        if is_typed(&prompt) {
            return Ok(once(ToolState::Allow));
        }
        let folder = self.folder_of(prompt.agent_id);
        let level = self.level_of(prompt.agent_id);
        if self.allowed_without_prompt(&prompt, folder.as_deref(), level) {
            return Ok(once(ToolState::Allow));
        }

        let reply_rx = self.start_waiting(&prompt);
        let always = always_offer(&prompt, folder.as_deref(), level);
        self.deliver_prompt(&prompt, always).await?;
        Ok(once(self.wait(&prompt, reply_rx).await))
    }
}

/// What a call would do, in the words the prompt shows.
pub(super) fn describe(tool: &str, args: &Value) -> String {
    let text = if tool == RUN_COMMAND_TOOL {
        describe_command(args)
    } else if let Some(path) = args.get("path").and_then(Value::as_str) {
        path.to_string()
    } else {
        args.to_string()
    };
    truncate_with_ellipsis(&text, MAX_DESCRIPTION_CHARS)
}

fn describe_command(args: &Value) -> String {
    let line = args
        .get("shell_script")
        .or_else(|| args.get("command"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| command_words(args).map(|words| words.join(" ")))
        .unwrap_or_else(|| args.to_string());
    let mut notes = Vec::new();
    if let Some(cwd) = args
        .get("cwd")
        .or_else(|| args.get("working_dir"))
        .and_then(Value::as_str)
    {
        notes.push(format!("in {cwd}"));
    }
    if args.get("background").and_then(Value::as_bool) == Some(true) {
        notes.push("in the background".to_string());
    }
    if notes.is_empty() {
        line
    } else {
        format!("{line} ({})", notes.join(", "))
    }
}
