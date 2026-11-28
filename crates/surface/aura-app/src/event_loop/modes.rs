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
use super::subagents::AgentHub;
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

/// What an agent's chat says when the user switches it to plan mode.
pub(super) const PLAN_NOTE: &str =
    "Plan mode: the agent researches and writes a plan instead of changing \
    code. /plan again returns to the previous mode. Say \"implement\" or run /build to implement \
    the plan.";

/// Whether commands can be confined to the agent's folder here, which
/// `yolo` relies on to let them run without asking.
pub(crate) fn commands_confinable() -> bool {
    aura_exec_tools::confine::supported()
}

/// Whether `tool` runs without asking under `permissions`. The mode's
/// session ceiling is checked first, so this never widens what the mode
/// allows.
fn tool_state(
    permissions: Permissions,
    tool: &str,
    extra: &BTreeMap<String, ToolModeClass>,
) -> ToolState {
    if tool == JOB_KILL_TOOL_NAME {
        // Stopping a background job only touches a job the agent started.
        return if permissions.approval == ApprovalLevel::Strict {
            ToolState::Ask
        } else {
            ToolState::Allow
        };
    }
    let class = tool_mode_class(tool).or_else(|| extra.get(tool).copied());
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
    // Integration tools, added at runtime, are classed by their effect.
    user_defaults_with(permissions, tools, &crate::integrations::tool_classes())
}

/// [`user_defaults`] with the integration tools' classes `extra`.
fn user_defaults_with(
    permissions: Permissions,
    tools: &[String],
    extra: &BTreeMap<String, ToolModeClass>,
) -> UserToolDefaults {
    if permissions.approval == ApprovalLevel::Yolo && commands_confinable() {
        return UserToolDefaults::full_access();
    }
    let per_tool: BTreeMap<String, ToolState> = tools
        .iter()
        .chain(extra.keys())
        .filter_map(|tool| {
            let state = tool_state(permissions, tool, extra);
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
    switch(state, agent_id, &kernel, permissions, PLAN_NOTE).await;
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
    let to = Permissions::new(mode, approval);
    switch(state, agent_id, kernel, to, PLAN_NOTE).await
}

/// Take `agent`, whose kernel isn't here (a cloud agent's runs on its
/// machine), out of plan mode into the mode it had before, as its next
/// turn's request to its machine then says. Its chat and every row it is
/// on show the new mode at once. Returns whether it was in plan mode.
pub(crate) async fn leave_plan_mode_of(hub: &AgentHub, agent: AgentId) -> bool {
    if hub.mode_of(agent) != PermissionMode::Plan {
        return false;
    }
    let back = hub.exit_plan_mode(agent);
    let approval = hub.permissions_of(agent).approval;
    hub.set_permissions(agent, Permissions::new(back, approval));
    info!(%agent, mode = %back, "Left plan mode");
    AgentUi::new(hub.commands().clone(), agent)
        .send(UiCommand::SetMode(back))
        .await;
    hub.send_agent_list().await;
    true
}

/// Whether a call of `tool` is implementation, not planning: it edits a
/// file, commits or pushes. A cloud machine doesn't hold an agent to plan
/// mode's ceiling, so its planning turns can do this.
pub(crate) fn implements(tool: &str) -> bool {
    matches!(
        tool_mode_class(tool),
        Some(ToolModeClass::FileWrite | ToolModeClass::WorkspaceMutation)
    ) || matches!(tool, "git_push" | "git_commit_push" | "git_ship")
}

/// [`switch_mode`] to plan mode for the reason `note` gives, which the
/// agent's chat shows instead of what it says when the user switches.
pub(super) async fn enter_plan_mode(
    state: &LoopState<'_>,
    agent_id: AgentId,
    kernel: &Kernel,
    note: &str,
) -> bool {
    let approval = state.hub.permissions_of(agent_id).approval;
    let to = Permissions::new(PermissionMode::Plan, approval);
    switch(state, agent_id, kernel, to, note).await
}

/// Switch `agent_id`, whose kernel is `kernel`, to `to`: record the
/// switch, then change the policy its kernel evaluates the next tool call
/// with; a switch into plan mode says `plan_note`. Reports failures itself
/// and returns `false`.
async fn switch(
    state: &LoopState<'_>,
    agent_id: AgentId,
    kernel: &Kernel,
    to: Permissions,
    plan_note: &str,
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
    announce_switch(state, &ui, agent_id, (from, to), plan_note).await;
    super::policies::send_policies_of(state, agent_id).await;
    if from.mode != to.mode {
        // Every row the agent is on marks plan mode, not only its chat.
        state.hub.send_agent_list().await;
    }
    true
}

/// Show what changed between `from` and `to` (mode and/or approval), with
/// `plan_note` for a switch into plan mode.
async fn announce_switch(
    state: &LoopState<'_>,
    ui: &AgentUi,
    agent_id: AgentId,
    (from, to): (Permissions, Permissions),
    plan_note: &str,
) {
    if from.mode != to.mode {
        ui.send(UiCommand::SetMode(to.mode)).await;
        ui.send(UiCommand::ShowSuccess(format!(
            "Mode: {} — {}",
            to.mode,
            to.mode.description()
        )))
        .await;
        if to.mode == PermissionMode::Plan {
            ui.send(system_message(plan_note.to_string())).await;
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

    /// Classes as the integrations service gives them for Runpod.
    fn integration_classes() -> BTreeMap<String, ToolModeClass> {
        [
            ("runpod_list_pods", ToolModeClass::Invoke),
            ("runpod_create_pod", ToolModeClass::Network),
            ("runpod_terminate_pod", ToolModeClass::Network),
        ]
        .into_iter()
        .map(|(tool, class)| (tool.to_string(), class))
        .collect()
    }

    #[test]
    fn integration_reads_run_without_asking_and_the_rest_ask() {
        let classes = integration_classes();
        let state = |permissions, tool: &str| {
            let defaults = user_defaults_with(permissions, &names(TOOLS), &classes);
            aura_core_types::resolve_effective_permission(&defaults, None, tool)
        };
        let ask = Permissions::new(PermissionMode::Agent, ApprovalLevel::Ask);
        assert_eq!(state(ask, "runpod_list_pods"), ToolState::Allow);
        assert_eq!(state(ask, "runpod_create_pod"), ToolState::Ask);
        assert_eq!(state(ask, "runpod_terminate_pod"), ToolState::Ask);
        let strict = Permissions::new(PermissionMode::Agent, ApprovalLevel::Strict);
        // Reads don't ask under strict either, as `read_file` doesn't.
        assert_eq!(state(strict, "runpod_list_pods"), ToolState::Allow);
        assert_eq!(state(strict, "runpod_create_pod"), ToolState::Ask);
        let plan = Permissions::new(PermissionMode::Plan, ApprovalLevel::Ask);
        assert_eq!(state(plan, "runpod_list_pods"), ToolState::Allow);
    }

    #[test]
    fn the_mode_gate_lets_integration_reads_into_plan_mode_only() {
        use aura_agent_kernel::{Policy, PolicyConfig};
        use aura_core_types::{ActionKind, Proposal, ToolCall};
        let policy = Policy::new(
            PolicyConfig::default()
                .with_mode(aura_core_modes::AgentMode::Plan)
                .with_tool_classes(integration_classes()),
        );
        let allowed = |tool: &str| {
            let call = ToolCall::new(tool, serde_json::json!({}));
            let payload = serde_json::to_vec(&call).unwrap();
            policy
                .check(&Proposal::new(
                    ActionKind::Delegate,
                    bytes::Bytes::from(payload),
                ))
                .allowed
        };
        assert!(allowed("runpod_list_pods"));
        assert!(!allowed("runpod_create_pod"));
        assert!(!allowed("runpod_terminate_pod"));
    }
}
