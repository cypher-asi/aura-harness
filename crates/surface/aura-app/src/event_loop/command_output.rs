//! Shows what an agent's foreground commands print while they run, in the
//! running call's block, or under the command the user typed.

use super::agent_events::AgentUi;
use super::run_command::is_user_tool_use;
use aura_core_types::AgentId;
use aura_exec_tools::CommandOutputHook;
use aura_ui::UiCommand;
use tokio::sync::mpsc;

/// Sends the output of one agent's commands to its chat.
pub struct TerminalCommandOutput {
    ui: AgentUi,
}

impl TerminalCommandOutput {
    #[must_use]
    pub fn new(commands: mpsc::Sender<UiCommand>, agent_id: AgentId) -> Self {
        Self {
            ui: AgentUi::new(commands, agent_id),
        }
    }
}

impl CommandOutputHook for TerminalCommandOutput {
    fn output(&self, tool_use_id: &str, chunk: &[u8]) {
        let chunk = String::from_utf8_lossy(chunk).into_owned();
        let command = if is_user_tool_use(tool_use_id) {
            UiCommand::CommandOutput { chunk }
        } else {
            UiCommand::ToolOutput {
                id: tool_use_id.to_string(),
                chunk,
            }
        };
        self.ui.try_send(command);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(rx: &mut mpsc::Receiver<UiCommand>) -> (String, UiCommand) {
        match rx.try_recv() {
            Ok(UiCommand::ForAgent { agent_id, command }) => (agent_id, *command),
            other => panic!("expected a command for an agent's chat, got {other:?}"),
        }
    }

    #[test]
    fn model_calls_get_tool_output_and_typed_commands_get_command_output() {
        let (tx, mut rx) = mpsc::channel(8);
        let agent = AgentId::generate();
        let hook = TerminalCommandOutput::new(tx, agent);

        hook.output("toolu_1", b"building\n");
        let (agent_id, command) = sent(&mut rx);
        assert_eq!(agent_id, agent.to_hex());
        assert!(
            matches!(&command, UiCommand::ToolOutput { id, chunk } if id == "toolu_1" && chunk == "building\n"),
            "{command:?}"
        );

        let typed = super::super::run_command::user_command_proposal("ls", None);
        hook.output(&typed.tool_use_id, b"a b\n");
        let (_, command) = sent(&mut rx);
        assert!(
            matches!(&command, UiCommand::CommandOutput { chunk } if chunk == "a b\n"),
            "{command:?}"
        );
    }

    #[test]
    fn output_is_dropped_rather_than_waited_on_when_the_ui_is_behind() {
        let (tx, mut rx) = mpsc::channel(1);
        let hook = TerminalCommandOutput::new(tx, AgentId::generate());
        hook.output("toolu_1", b"one");
        hook.output("toolu_1", b"two");
        let (_, command) = sent(&mut rx);
        assert!(matches!(command, UiCommand::ToolOutput { chunk, .. } if chunk == "one"));
        assert!(rx.try_recv().is_err());
    }
}
