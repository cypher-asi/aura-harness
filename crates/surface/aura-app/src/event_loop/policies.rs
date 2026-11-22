//! The POLICIES tab: the policy an agent's kernel enforces, as it stands.
//!
//! [`send_policies_of`] sends the agent a [`PolicyView`] (wrapped in
//! `UiCommand::ForAgent`) built from its kernel's [`Policy`]: the work
//! mode and approval level, how each tool offered to it resolves before
//! its arguments, its capabilities and scope, the project's "always
//! allow" rules and how `ask` calls reach the user. The view is a
//! snapshot, so it is sent again whenever what it shows may have changed:
//!
//! - when the agent is shown (at startup and when the user switches to it),
//! - after its mode or approval level changes,
//! - after an "always allow" answer adds a project rule,
//! - and when the UI asks for it (`UiEvent::RequestPolicies`).
//!
//! An agent with no open kernel has no policy to show and gets nothing.

use super::agent_events::AgentUi;
use super::modes::{self, Permissions};
use super::LoopState;
use aura_agent_kernel::{Policy, PolicyVerdict};
use aura_core_types::{
    AgentId, AgentPermissions, AgentScope, InstalledIntegrationDefinition, ToolState,
};
use aura_ui_terminal::events::{PolicyView, ToolPolicyState, ToolPolicyView};
use aura_ui_terminal::UiCommand;
use tracing::debug;

/// How the terminal answers an agent's `ask` calls, which the policy
/// itself doesn't know.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LiveApprovals {
    /// Labels of the project's "always allow" rules
    pub always_allow: Vec<String>,
    /// Whether a prompter is attached, so `ask` calls reach the user
    pub live_prompts: bool,
    /// How long a prompt waits before its call is denied, in seconds
    pub prompt_timeout_secs: u64,
    /// `PermissionRequest` plugin hooks consulted before prompting
    pub permission_hooks: usize,
}

/// The POLICIES view of `policy` for an agent under `permissions` that is
/// offered `tools`. Tools are listed once each, in name order.
pub(crate) fn policy_view(
    policy: &Policy,
    permissions: Permissions,
    tools: &[String],
    live: LiveApprovals,
) -> PolicyView {
    let config = policy.config();
    let mut action_kinds: Vec<String> = config
        .allowed_action_kinds
        .iter()
        .map(|kind| format!("{kind:?}").to_ascii_lowercase())
        .collect();
    action_kinds.sort_unstable();
    PolicyView {
        mode: permissions.mode,
        approval: permissions.approval,
        // Hashed from the same snapshot the rest of the view reads.
        policy_hash: config.snapshot_hash(),
        action_kinds,
        max_proposals: config.max_proposals,
        capabilities: config
            .agent_permissions
            .capabilities
            .iter()
            .map(|capability| format!("{capability:?}"))
            .collect(),
        capability_total: AgentPermissions::full_access().capabilities.len(),
        scope: scope_text(&config.agent_permissions.scope),
        tools: tool_views(policy, tools),
        always_allow: live.always_allow,
        session_grants: session_grants(policy),
        integrations: config
            .installed_integrations
            .iter()
            .map(integration_label)
            .collect(),
        commands_confined: modes::commands_confinable(),
        live_prompts: live.live_prompts,
        prompt_timeout_secs: live.prompt_timeout_secs,
        permission_hooks: live.permission_hooks,
    }
}

/// Send `agent_id` the policy its kernel enforces now. Sends nothing when
/// it has no open kernel.
pub(super) async fn send_policies_of(state: &LoopState<'_>, agent_id: AgentId) {
    let Some(kernel) = state.kernel_for(agent_id) else {
        debug!(%agent_id, "No open kernel to show the policy of");
        return;
    };
    let live = LiveApprovals {
        always_allow: state.approvals.rules_for(agent_id),
        live_prompts: kernel.has_approval_prompter(),
        prompt_timeout_secs: state.approvals.timeout().as_secs(),
        permission_hooks: kernel.permission_hook_count(),
    };
    let view = policy_view(
        kernel.policy(),
        state.hub.permissions_of(agent_id),
        &state.hub.tool_names(),
        live,
    );
    AgentUi::new(state.commands.clone(), agent_id)
        .send(UiCommand::SetPolicies(view))
        .await;
}

/// How each of `tools` resolves under `policy`, deduplicated and in name
/// order.
fn tool_views(policy: &Policy, tools: &[String]) -> Vec<ToolPolicyView> {
    let mut names: Vec<&str> = tools.iter().map(String::as_str).collect();
    names.sort_unstable();
    names.dedup();
    names
        .into_iter()
        .map(|name| {
            let evaluation = policy.explain_tool(name);
            ToolPolicyView {
                name: name.to_string(),
                state: verdict_state(&evaluation.verdict),
                decided_by: evaluation.decided_by.to_string(),
            }
        })
        .collect()
}

const fn verdict_state(verdict: &PolicyVerdict) -> ToolPolicyState {
    match verdict {
        PolicyVerdict::Allow => ToolPolicyState::On,
        PolicyVerdict::RequireApproval { .. } => ToolPolicyState::Ask,
        PolicyVerdict::Deny { .. } => ToolPolicyState::Off,
    }
}

const fn tool_state(state: ToolState) -> ToolPolicyState {
    match state {
        ToolState::Allow => ToolPolicyState::On,
        ToolState::Ask => ToolPolicyState::Ask,
        ToolState::Deny => ToolPolicyState::Off,
    }
}

/// Answers remembered for the session, as `tool=state` in tool order.
fn session_grants(policy: &Policy) -> Vec<String> {
    policy
        .session_tool_states()
        .into_iter()
        .map(|(tool, state)| format!("{tool}={}", tool_state(state).label()))
        .collect()
}

/// `scope` in words: "orgs: a, b · projects: p · agents: any".
fn scope_text(scope: &AgentScope) -> String {
    if scope.is_universe() {
        return "all orgs, projects and agents".to_string();
    }
    let axis = |name: &str, values: &[String]| {
        if values.is_empty() {
            format!("{name}: any")
        } else {
            format!("{name}: {}", values.join(", "))
        }
    };
    format!(
        "{} · {} · {}",
        axis("orgs", &scope.orgs),
        axis("projects", &scope.projects),
        axis("agents", &scope.agent_ids)
    )
}

/// An installed integration in a few words: "GitHub (github oauth)".
fn integration_label(integration: &InstalledIntegrationDefinition) -> String {
    let name = if integration.name.is_empty() {
        &integration.integration_id
    } else {
        &integration.name
    };
    let details: Vec<&str> = [integration.provider.as_str(), integration.kind.as_str()]
        .into_iter()
        .filter(|detail| !detail.is_empty())
        .collect();
    if details.is_empty() {
        name.clone()
    } else {
        format!("{name} ({})", details.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_ui_terminal::{ApprovalLevel, PermissionMode};
    use std::collections::HashMap;

    fn tools() -> Vec<String> {
        ["write_file", "read_file", "run_command", "read_file"]
            .iter()
            .map(|tool| (*tool).to_string())
            .collect()
    }

    fn view(mode: PermissionMode, approval: ApprovalLevel) -> PolicyView {
        let permissions = Permissions::new(mode, approval);
        let tools = tools();
        let policy = Policy::new(crate::harness::mode_policy(permissions, &tools));
        policy_view(&policy, permissions, &tools, LiveApprovals::default())
    }

    fn tool<'a>(view: &'a PolicyView, name: &str) -> &'a ToolPolicyView {
        view.tools
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("no {name} in {:?}", view.tools))
    }

    #[test]
    fn agent_ask_runs_reads_and_asks_before_writes() {
        let view = view(PermissionMode::Agent, ApprovalLevel::Ask);
        assert_eq!(view.mode, PermissionMode::Agent);
        assert_eq!(view.approval, ApprovalLevel::Ask);
        let names: Vec<&str> = view.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["read_file", "run_command", "write_file"]);

        let read = tool(&view, "read_file");
        assert_eq!(read.state, ToolPolicyState::On);
        assert!(
            read.decided_by.starts_with("user_default."),
            "{}",
            read.decided_by
        );
        assert_eq!(tool(&view, "write_file").state, ToolPolicyState::Ask);
        assert!(!view.policy_hash.is_empty());
    }

    #[test]
    fn plan_mode_turns_writes_off() {
        let view = view(PermissionMode::Plan, ApprovalLevel::Strict);
        let write = tool(&view, "write_file");
        assert_eq!(write.state, ToolPolicyState::Off);
        assert!(
            write.decided_by.starts_with("mode."),
            "{}",
            write.decided_by
        );
        assert_eq!(tool(&view, "read_file").state, ToolPolicyState::On);
    }

    #[test]
    fn yolo_runs_writes_without_asking() {
        let view = view(PermissionMode::Agent, ApprovalLevel::Yolo);
        assert_eq!(tool(&view, "write_file").state, ToolPolicyState::On);
    }

    #[test]
    fn the_default_bundle_holds_every_capability_everywhere() {
        let view = view(PermissionMode::Agent, ApprovalLevel::Ask);
        assert_eq!(view.capabilities.len(), view.capability_total);
        assert!(view.capabilities.iter().any(|c| c == "SpawnAgent"));
        assert_eq!(view.scope, "all orgs, projects and agents");
        let mut sorted = view.action_kinds.clone();
        sorted.sort();
        assert_eq!(view.action_kinds, sorted);
        assert!(view.action_kinds.iter().any(|kind| kind == "delegate"));
        assert_eq!(view.commands_confined, modes::commands_confinable());
    }

    #[test]
    fn live_approvals_and_session_grants_are_shown() {
        let permissions = Permissions::default();
        let tools = tools();
        let policy = Policy::new(crate::harness::mode_policy(permissions, &tools));
        policy.remember_tool_state_for_session("write_file", ToolState::Allow);
        policy.remember_tool_state_for_session("run_command", ToolState::Deny);
        let live = LiveApprovals {
            always_allow: vec!["`cargo test` commands".to_string()],
            live_prompts: true,
            prompt_timeout_secs: 600,
            permission_hooks: 2,
        };
        let view = policy_view(&policy, permissions, &tools, live);
        assert_eq!(view.session_grants, ["run_command=off", "write_file=on"]);
        assert_eq!(view.always_allow, ["`cargo test` commands"]);
        assert!(view.live_prompts);
        assert_eq!(view.prompt_timeout_secs, 600);
        assert_eq!(view.permission_hooks, 2);
        assert_eq!(tool(&view, "write_file").state, ToolPolicyState::On);
    }

    #[test]
    fn a_narrowed_scope_names_each_axis() {
        let scope = AgentScope {
            orgs: vec!["a".to_string(), "b".to_string()],
            projects: vec!["p".to_string()],
            agent_ids: Vec::new(),
        };
        assert_eq!(scope_text(&scope), "orgs: a, b · projects: p · agents: any");
    }

    #[test]
    fn integrations_are_labelled_by_name_provider_and_kind() {
        let integration = |name: &str, provider: &str, kind: &str| InstalledIntegrationDefinition {
            integration_id: "int-1".to_string(),
            name: name.to_string(),
            provider: provider.to_string(),
            kind: kind.to_string(),
            metadata: HashMap::new(),
        };
        assert_eq!(
            integration_label(&integration("GitHub", "github", "oauth")),
            "GitHub (github oauth)"
        );
        assert_eq!(integration_label(&integration("", "", "")), "int-1");
    }
}
