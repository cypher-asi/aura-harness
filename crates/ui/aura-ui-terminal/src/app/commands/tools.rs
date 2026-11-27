//! Tool calls, shell commands and boot consoles shown live in the chat.

use std::mem;

use crate::app::chat_view::EarlyPolicy;
use crate::app::App;
use crate::components::{
    elapsed_ms, Activity, BootConsole, Message, MessageRole, ToolStatus, ToolStep,
};
use crate::events::{BootUpdate, UiCommand};

impl App {
    /// Apply a command about a tool call, a shell command or a boot
    /// console; any other comes back.
    pub(super) fn tool_ui_command(&mut self, cmd: UiCommand) -> Option<UiCommand> {
        match cmd {
            UiCommand::ShowTool(data) => {
                self.add_activity(Activity::tool(&data.id, &data.name, &data.args));
                self.attach_early_policy(&data.id);
            }
            UiCommand::UpdateToolArgs { id, args } => self.cmd_update_tool_args(&id, args),
            UiCommand::ToolPolicy { id, tool, decision } => {
                self.cmd_tool_policy(&id, &tool, decision);
            }
            UiCommand::ToolRetrying {
                id,
                attempt,
                max_attempts,
            } => self.cmd_tool_retrying(&id, attempt, max_attempts),
            UiCommand::CompleteTool {
                id,
                result,
                success,
            } => self.cmd_complete_tool(&id, &result, success),
            UiCommand::ToolOutput { id, chunk } => self.cmd_tool_output(&id, &chunk),
            UiCommand::ToolDiff { id, diff } => self.cmd_tool_diff(&id, diff),
            UiCommand::ShowDiff { title, files } => self.add_message(Message::diff(&title, files)),
            UiCommand::CommandOutput { chunk } => self.cmd_command_output(&chunk),
            UiCommand::Boot { id, update } => self.cmd_boot(&id, update),
            UiCommand::CommandFinished { output, exit_code } => {
                self.cmd_command_finished(&output, exit_code);
            }
            other => return Some(other),
        }
        None
    }

    /// Apply `update` to the tool call with use id `id`; `false` when no
    /// such call is shown.
    pub(super) fn update_tool(&mut self, id: &str, update: impl FnOnce(&mut ToolStep)) -> bool {
        let found = self
            .messages
            .iter_mut()
            .rev()
            .find(|m| matches!(m.activity_step(), Some(Activity::Tool(tool)) if tool.id == id));
        let Some(message) = found else {
            return false;
        };
        message.update_activity(|activity| {
            if let Activity::Tool(tool) = activity {
                update(tool);
            }
        });
        true
    }

    fn cmd_update_tool_args(&mut self, id: &str, args: String) {
        self.update_tool(id, |tool| tool.args = args);
    }

    /// Keep the policy decision with its tool call, so a run of calls
    /// stays one block. While a turn runs, a call not shown yet may still
    /// come and the decision waits for it; otherwise it gets a `policy:`
    /// line.
    fn cmd_tool_policy(&mut self, id: &str, tool: &str, decision: String) {
        if self.update_tool(id, |step| step.policy = Some(decision.clone())) {
            return;
        }
        if std::env::var_os("OLD_POLICY").is_none() && self.is_busy() {
            self.chat.early_policies.push(EarlyPolicy {
                id: id.to_string(),
                tool: tool.to_string(),
                decision,
            });
        } else {
            self.add_policy_line(tool, &decision);
        }
    }

    /// Give the call `id`, just shown, the decision that came before it.
    fn attach_early_policy(&mut self, id: &str) {
        let early = &mut self.chat.early_policies;
        let Some(at) = early.iter().position(|policy| policy.id == id) else {
            return;
        };
        let decision = early.remove(at).decision;
        self.update_tool(id, |step| step.policy = Some(decision));
    }

    /// Show the decisions whose calls never came as `policy:` lines.
    pub(super) fn show_early_policies(&mut self) {
        for policy in mem::take(&mut self.chat.early_policies) {
            self.add_policy_line(&policy.tool, &policy.decision);
        }
    }

    fn add_policy_line(&mut self, tool: &str, decision: &str) {
        let line = format!("policy: {tool} {decision}");
        self.add_message(Message::new(MessageRole::System, &line));
    }

    fn cmd_tool_retrying(&mut self, id: &str, attempt: u32, max_attempts: u32) {
        self.update_tool(id, |tool| {
            tool.retry = Some((attempt, max_attempts));
            tool.live.clear();
        });
    }

    fn cmd_complete_tool(&mut self, id: &str, result: &str, success: bool) {
        self.update_tool(id, |tool| {
            tool.result = Some(crate::app::format::format_tool_result(&tool.name, result));
            tool.status = if success {
                ToolStatus::Success
            } else {
                ToolStatus::Error
            };
            tool.retry = None;
            tool.duration_ms = elapsed_ms(tool.started);
            tool.live.clear();
        });
        self.refresh_sandbox();
    }

    fn cmd_tool_output(&mut self, id: &str, chunk: &str) {
        self.update_tool(id, |tool| {
            if tool.status == ToolStatus::Running {
                tool.live.push(chunk);
            }
        });
    }

    fn cmd_tool_diff(&mut self, id: &str, diff: String) {
        self.update_tool(id, |tool| tool.diff = Some(diff));
    }

    fn cmd_command_output(&mut self, chunk: &str) {
        if let Some(message) = self
            .messages
            .iter_mut()
            .rev()
            .find(|m| m.is_running_shell())
        {
            message.append_shell_output(chunk);
        }
    }

    fn cmd_command_finished(&mut self, output: &str, exit_code: Option<i32>) {
        if let Some(message) = self
            .messages
            .iter_mut()
            .rev()
            .find(|m| m.is_running_shell())
        {
            message.finish_shell(output, exit_code);
        }
        self.refresh_sandbox();
    }

    /// Change boot console `id`, adding it on its first update.
    fn cmd_boot(&mut self, id: &str, update: BootUpdate) {
        if let Some(message) = self
            .messages
            .iter_mut()
            .rev()
            .find(|m| m.boot_console().is_some_and(|console| console.id == id))
        {
            message.update_boot(|console| console.apply(update));
            return;
        }
        let title = match &update {
            BootUpdate::Begin { title } => title.as_str(),
            _ => "Cloud machine",
        };
        let mut console = BootConsole::new(id, title);
        console.apply(update);
        self.add_message(Message::boot(console));
    }
}
