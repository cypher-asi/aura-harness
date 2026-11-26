//! Work modes and approval levels: what an agent may do, and which of its
//! tool calls ask first.
//!
//! Each agent has its own [`PermissionMode`] and [`ApprovalLevel`], kept
//! in the [`AgentHub`](super::AgentHub) so spawned agents start with their
//! parent's. Together they become the kernel policy's session-mode
//! ceiling, its approval level, and a per-tool `on`/`ask` table, applied
//! to the agent's kernel before every piece of work and straight away
//! when the user switches, so the switch takes effect at the agent's next
//! tool call even mid-turn. A switch goes through
//! `Kernel::switch_permissions`, which records one `mode_change` entry
//! naming both tables (`agent/ask` → `agent/yolo`) before the policy
//! changes. `plan` is the terminal's plan mode (`/plan`; see
//! [`super::plan_mode`]).
//!
//! The levels:
//!
//! - `ask`: reads run; everything else asks, unless the project's "always
//!   allow" rules cover it (see [`super::approvals`]).
//! - `strict`: like `ask`, but the rules are ignored and stopping a
//!   background job asks too.
//! - `yolo`: nothing asks, and the agent's commands may only write inside
//!   its folder (see [`aura_exec_tools::confine`]). Where commands can't
//!   be confined they still ask, unless they plainly stay in the folder.

use super::agent_events::AgentUi;
use super::handlers::system_message;
use super::LoopState;
use aura_agent_kernel::{tool_mode_class, Kernel, ToolModeClass};
use aura_core_types::{AgentId, ToolState, UserToolDefaults};
use aura_exec_tools::jobs::JOB_KILL_TOOL_NAME;
use aura_ui::{ApprovalLevel, PermissionMode, UiCommand};
use std::collections::BTreeMap;
use tracing::{info, warn};

/// What decides an agent's tool calls: its work mode and approval level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Permissions {
    pub mode: PermissionMode,
    pub approval: ApprovalLevel,
}

impl Permissions {
    pub(crate) const fn new(mode: PermissionMode, approval: ApprovalLevel) -> Self {
        Self { mode, approval }
    }

    /// Name of the permission table in the record: `agent/ask`.
    pub(crate) fn name(self) -> String {
        format!("{}/{}", self.mode.name(), self.approval.name())
    }
}

/// Whether commands can be confined to the agent's folder here, which
/// `yolo` relies on to let them run without asking.
pub(crate) fn commands_confinable() -> bool {
    aura_exec_tools::confine::supported()
}

/// Whether `tool` runs without asking under `permissions`. The mode's
/// session ceiling is checked first, so this never widens what the mode
/// allows.
fn tool_state(permissions: Permissions, tool: &str) -> ToolState {
    if tool == JOB_KILL_TOOL_NAME {
        // Stopping a background job only touches a job the agent started.
        return if permissions.approval == ApprovalLevel::Strict {
            ToolState::Ask
        } else {
            ToolState::Allow
        };
    }
    let class = tool_mode_class(tool);
    if permissions.approval == ApprovalLevel::Yolo {
        let unconfined_command = class == Some(ToolModeClass::Subprocess) && !commands_confinable();
        return if unconfined_command {
            ToolState::Ask
        } else {
            ToolState::Allow
        };
    }
    match class {
        Some(ToolModeClass::Read | ToolModeClass::Invoke) => ToolState::Allow,
        _ => ToolState::Ask,
    }
}

/// The user tool defaults of `permissions` for an agent that has `tools`.
/// Tools not listed, such as ones installed at runtime, ask, except under
/// `yolo` with confinable commands, where everything runs.
pub(crate) fn user_defaults(permissions: Permissions, tools: &[String]) -> UserToolDefaults {
    if permissions.approval == ApprovalLevel::Yolo && commands_confinable() {
        return UserToolDefaults::full_access();
    }
    let per_tool: BTreeMap<String, ToolState> = tools
        .iter()
        .filter_map(|tool| {
            let state = tool_state(permissions, tool);
            (state == ToolState::Allow).then(|| (tool.clone(), state))
        })
        .collect();
    UserToolDefaults::default_permissions(per_tool, ToolState::Ask)
}

/// Make `kernel` evaluate its next tool calls under `permissions`.
pub(crate) fn apply(kernel: &Kernel, permissions: Permissions, tools: &[String]) {
    let defaults = user_defaults(permissions, tools);
    kernel.policy().reconfigure(|config| {
        config.mode = permissions.mode.agent_mode();
        config.approval = permissions.approval;
        config.user_default = defaults;
    });
}

/// Show `agent_id` its mode and approval level.
pub(super) async fn send_mode(state: &LoopState<'_>, agent_id: AgentId) {
    let permissions = state.hub.permissions_of(agent_id);
    let ui = AgentUi::new(state.commands.clone(), agent_id);
    ui.send(UiCommand::SetMode(permissions.mode)).await;
    ui.send(UiCommand::SetApproval(permissions.approval)).await;
}

/// Switch `agent_id` to `mode` (`/mode`, `/plan`), whether it is idle or
/// at work.
pub(super) async fn set_mode(state: &mut LoopState<'_>, agent_id: AgentId, mode: PermissionMode) {
    let approval = state.hub.permissions_of(agent_id).approval;
    set_permissions(state, agent_id, Permissions::new(mode, approval)).await;
}

/// Switch `agent_id` to approval `level` (`/approval`, Shift+Tab), whether
/// it is idle or at work.
pub(super) async fn set_approval(
    state: &mut LoopState<'_>,
    agent_id: AgentId,
    level: ApprovalLevel,
) {
    let mode = state.hub.permissions_of(agent_id).mode;
    set_permissions(state, agent_id, Permissions::new(mode, level)).await;
}

async fn set_permissions(state: &LoopState<'_>, agent_id: AgentId, permissions: Permissions) {
    let Some(kernel) = state.kernel_for(agent_id) else {
        AgentUi::new(state.commands.clone(), agent_id)
            .send(UiCommand::ShowError(
                "No open session to change the mode of".to_string(),
            ))
            .await;
        return;
    };
    switch(state, agent_id, &kernel, permissions).await;
}

/// Switch `agent_id`, whose kernel is `kernel`, to work `mode`, keeping its
/// approval level. Reports failures itself and returns `false`.
pub(super) async fn switch_mode(
    state: &LoopState<'_>,
    agent_id: AgentId,
    kernel: &Kernel,
    mode: PermissionMode,
) -> bool {
    let approval = state.hub.permissions_of(agent_id).approval;
    switch(state, agent_id, kernel, Permissions::new(mode, approval)).await
}

/// Switch `agent_id`, whose kernel is `kernel`, to `to`: record the
/// switch, then change the policy its kernel evaluates the next tool call
/// with. Reports failures itself and returns `false`.
async fn switch(
    state: &LoopState<'_>,
    agent_id: AgentId,
    kernel: &Kernel,
    to: Permissions,
) -> bool {
    let ui = AgentUi::new(state.commands.clone(), agent_id);
    let from = state.hub.permissions_of(agent_id);
    if from == to {
        send_mode(state, agent_id).await;
        return true;
    }
    let defaults = user_defaults(to, &state.hub.tool_names());
    if let Err(e) = kernel
        .switch_permissions(
            to.mode.agent_mode(),
            to.approval,
            defaults,
            &from.name(),
            &to.name(),
        )
        .await
    {
        warn!(%agent_id, error = %e, "Failed to record the mode change via kernel");
        ui.send(UiCommand::ShowError(format!(
            "Could not record the mode change, so it was not made: {e}"
        )))
        .await;
        return false;
    }
    state.hub.set_permissions(agent_id, to);
    info!(%agent_id, from = %from.name(), to = %to.name(), "Changed permissions");
    if from.mode != to.mode {
        ui.send(UiCommand::SetMode(to.mode)).await;
        ui.send(UiCommand::ShowSuccess(format!(
            "Mode: {} — {}",
            to.mode,
            to.mode.description()
        )))
        .await;
        if to.mode == PermissionMode::Plan {
            ui.send(system_message(
                "Plan mode: the agent researches and writes a plan instead of changing code. \
                 /plan again returns to the previous mode. Say \"implement\" or run /build to \
                 implement the plan."
                    .to_string(),
            ))
            .await;
        }
    }
    if from.approval != to.approval {
        ui.send(UiCommand::SetApproval(to.approval)).await;
        ui.send(UiCommand::ShowSuccess(format!(
            "Approval: {} — {}",
            to.approval,
            to.approval.description()
        )))
        .await;
        if to.approval == ApprovalLevel::Yolo {
            warn_if_unconfined(state, agent_id).await;
        }
    }
    super::policies::send_policies_of(state, agent_id).await;
    true
}

/// Tell `agent_id` when it is at `yolo` but its commands can't be confined.
pub(super) async fn warn_if_unconfined(state: &LoopState<'_>, agent_id: AgentId) {
    if state.hub.permissions_of(agent_id).approval != ApprovalLevel::Yolo || commands_confinable() {
        return;
    }
    AgentUi::new(state.commands.clone(), agent_id)
        .send(UiCommand::ShowWarning(
            "Commands can't be confined to this folder here (Linux Landlock is \
             unavailable), so commands that may reach outside it still ask"
                .to_string(),
        ))
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(tools: &[&str]) -> Vec<String> {
        tools.iter().map(|t| (*t).to_string()).collect()
    }

    fn states(permissions: Permissions, tools: &[&str]) -> impl Fn(&str) -> ToolState {
        let defaults = user_defaults(permissions, &names(tools));
        move |tool| aura_core_types::resolve_effective_permission(&defaults, None, tool)
    }

    const TOOLS: &[&str] = &[
        "read_file",
        "write_file",
        "edit_file",
        "run_command",
        "task",
        "todo_write",
        JOB_KILL_TOOL_NAME,
    ];

    #[test]
    fn ask_asks_for_everything_but_reads() {
        let state = states(
            Permissions::new(PermissionMode::Agent, ApprovalLevel::Ask),
            TOOLS,
        );
        assert_eq!(state("read_file"), ToolState::Allow);
        assert_eq!(state("todo_write"), ToolState::Allow);
        assert_eq!(state(JOB_KILL_TOOL_NAME), ToolState::Allow);
        assert_eq!(state("write_file"), ToolState::Ask);
        assert_eq!(state("run_command"), ToolState::Ask);
        assert_eq!(state("task"), ToolState::Ask);
        assert_eq!(state("mcp_unknown"), ToolState::Ask);
    }

    #[test]
    fn strict_also_asks_before_stopping_a_job() {
        let state = states(
            Permissions::new(PermissionMode::Agent, ApprovalLevel::Strict),
            TOOLS,
        );
        assert_eq!(state("read_file"), ToolState::Allow);
        assert_eq!(state("todo_write"), ToolState::Allow);
        assert_eq!(state(JOB_KILL_TOOL_NAME), ToolState::Ask);
        assert_eq!(state("edit_file"), ToolState::Ask);
        assert_eq!(state("run_command"), ToolState::Ask);
    }

    #[test]
    fn yolo_asks_nothing_when_commands_are_confined() {
        let state = states(
            Permissions::new(PermissionMode::Agent, ApprovalLevel::Yolo),
            TOOLS,
        );
        assert_eq!(state("write_file"), ToolState::Allow);
        assert_eq!(state("task"), ToolState::Allow);
        let command = if commands_confinable() {
            ToolState::Allow
        } else {
            ToolState::Ask
        };
        assert_eq!(state("run_command"), command);
    }

    #[test]
    fn permissions_are_named_mode_slash_level() {
        assert_eq!(
            Permissions::new(PermissionMode::Plan, ApprovalLevel::Strict).name(),
            "plan/strict"
        );
        assert_eq!(Permissions::default().name(), "agent/ask");
    }
}
