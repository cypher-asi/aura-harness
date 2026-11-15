//! Permission modes: which tool calls an agent makes without asking.
//!
//! Each agent has its own [`PermissionMode`], kept in the
//! [`AgentHub`](super::AgentHub) so spawned agents start in their parent's
//! mode. A mode becomes the kernel policy's session-mode ceiling plus a
//! per-tool `on`/`ask` table, applied to the agent's kernel before every
//! piece of work and straight away when the user switches, so the switch
//! takes effect at the agent's next tool call even mid-turn. A switch is
//! recorded as a `System` entry before the policy changes.

use super::agent_events::AgentUi;
use super::LoopState;
use aura_agent_kernel::{tool_mode_class, Kernel, ToolModeClass};
use aura_core_types::{
    AgentId, SystemKind, ToolState, Transaction, TransactionType, UserToolDefaults,
};
use aura_terminal::{PermissionMode, UiCommand};
use aura_tools::jobs::JOB_KILL_TOOL_NAME;
use std::collections::BTreeMap;
use tracing::{info, warn};

/// Whether `tool` runs without asking in `mode`. The mode's session
/// ceiling is checked first, so this never widens what the mode allows.
fn tool_state(mode: PermissionMode, tool: &str) -> ToolState {
    if mode == PermissionMode::Unrestricted {
        return ToolState::Allow;
    }
    // Stopping a background job only touches a job the agent started.
    if tool == JOB_KILL_TOOL_NAME {
        return ToolState::Allow;
    }
    match tool_mode_class(tool) {
        Some(ToolModeClass::Read | ToolModeClass::Invoke) => ToolState::Allow,
        Some(ToolModeClass::FileWrite | ToolModeClass::MarkdownDocument)
            if mode == PermissionMode::AcceptEdits =>
        {
            ToolState::Allow
        }
        _ => ToolState::Ask,
    }
}

/// The user tool defaults of `mode` for an agent that has `tools`. Tools
/// not listed, such as ones installed at runtime, ask.
pub(crate) fn user_defaults(mode: PermissionMode, tools: &[String]) -> UserToolDefaults {
    if mode == PermissionMode::Unrestricted {
        return UserToolDefaults::full_access();
    }
    let per_tool: BTreeMap<String, ToolState> = tools
        .iter()
        .filter_map(|tool| {
            let state = tool_state(mode, tool);
            (state == ToolState::Allow).then(|| (tool.clone(), state))
        })
        .collect();
    UserToolDefaults::default_permissions(per_tool, ToolState::Ask)
}

/// Make `kernel` evaluate its next tool calls under `mode`.
pub(crate) fn apply(kernel: &Kernel, mode: PermissionMode, tools: &[String]) {
    let defaults = user_defaults(mode, tools);
    kernel.policy().reconfigure(|config| {
        config.mode = mode.agent_mode();
        config.user_default = defaults;
    });
}

/// Show `agent_id` its mode.
pub(super) async fn send_mode(state: &LoopState<'_>, agent_id: AgentId) {
    AgentUi::new(state.commands.clone(), agent_id)
        .send(UiCommand::SetMode(state.hub.mode_of(agent_id)))
        .await;
}

/// Switch `agent_id` to `mode`: record the switch, then change the policy
/// its kernel evaluates the next tool call with.
pub(super) async fn set_mode(state: &mut LoopState<'_>, agent_id: AgentId, mode: PermissionMode) {
    let ui = AgentUi::new(state.commands.clone(), agent_id);
    let from = state.hub.mode_of(agent_id);
    if from == mode {
        send_mode(state, agent_id).await;
        return;
    }
    let Some(kernel) = state.kernel_for(agent_id) else {
        ui.send(UiCommand::ShowError(
            "No open session to change the mode of".to_string(),
        ))
        .await;
        return;
    };
    if let Err(problem) = record_mode_change(&kernel, agent_id, from, mode).await {
        ui.send(UiCommand::ShowError(problem)).await;
        return;
    }
    state.hub.set_mode(agent_id, mode);
    apply(&kernel, mode, &state.hub.tool_names());
    info!(%agent_id, %from, to = %mode, "Changed permission mode");
    ui.send(UiCommand::SetMode(mode)).await;
    ui.send(UiCommand::ShowSuccess(format!(
        "Mode: {mode} — {}",
        mode.description()
    )))
    .await;
}

/// Append the `System` entry for a mode switch to the agent's record. The
/// architecture requires it before any tool call runs under the new mode.
async fn record_mode_change(
    kernel: &Kernel,
    agent_id: AgentId,
    from: PermissionMode,
    to: PermissionMode,
) -> Result<(), String> {
    let payload = serde_json::json!({
        "system_kind": SystemKind::Infrastructure,
        "action": "permission_mode",
        "from": from.name(),
        "to": to.name(),
        "agent_mode": format!("{:?}", to.agent_mode()).to_ascii_lowercase(),
    });
    let bytes = serde_json::to_vec(&payload).map_err(|e| {
        warn!(error = %e, "Failed to serialize the mode change");
        format!("Could not record the mode change: {e}")
    })?;
    let tx = Transaction::new_chained(agent_id, TransactionType::System, bytes, None);
    kernel.process_direct(tx).await.map(|_| ()).map_err(|e| {
        warn!(error = %e, "Failed to record the mode change via kernel");
        format!("Could not record the mode change, so it was not made: {e}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(tools: &[&str]) -> Vec<String> {
        tools.iter().map(|t| (*t).to_string()).collect()
    }

    #[test]
    fn default_asks_for_everything_but_reads() {
        let tools = names(&[
            "read_file",
            "write_file",
            "run_command",
            "task",
            "todo_write",
        ]);
        let defaults = user_defaults(PermissionMode::Default, &tools);
        let state = |tool| aura_core_types::resolve_effective_permission(&defaults, None, tool);
        assert_eq!(state("read_file"), ToolState::Allow);
        assert_eq!(state("todo_write"), ToolState::Allow);
        assert_eq!(state("write_file"), ToolState::Ask);
        assert_eq!(state("run_command"), ToolState::Ask);
        assert_eq!(state("task"), ToolState::Ask);
        assert_eq!(state("mcp_unknown"), ToolState::Ask);
    }

    #[test]
    fn accept_edits_allows_file_edits_but_not_commands() {
        let tools = names(&["edit_file", "write_file", "delete_file", "run_command"]);
        let defaults = user_defaults(PermissionMode::AcceptEdits, &tools);
        let state = |tool| aura_core_types::resolve_effective_permission(&defaults, None, tool);
        assert_eq!(state("edit_file"), ToolState::Allow);
        assert_eq!(state("delete_file"), ToolState::Allow);
        assert_eq!(state("run_command"), ToolState::Ask);
    }

    #[test]
    fn unrestricted_asks_nothing() {
        let defaults = user_defaults(PermissionMode::Unrestricted, &[]);
        assert_eq!(
            aura_core_types::resolve_effective_permission(&defaults, None, "run_command"),
            ToolState::Allow
        );
    }
}
