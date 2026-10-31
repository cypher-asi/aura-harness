//! How a child agent's policy, tool permissions, and loop config are
//! derived from its parent.

use aura_agent::AgentLoopConfig;
use aura_agent_kernel::PolicyConfig;
use aura_core_types::{
    resolve_effective_permission, AgentMode, AgentPermissions, AgentToolPermissions,
    SubagentKindSpec, ToolState, UserDefaultMode, UserToolDefaults,
};
use std::collections::BTreeMap;

use crate::scheduler::AgentIdentity;

/// Per-tool permissions limited to `allowed_tools`, each at most what
/// the parent has for that tool.
pub(super) fn narrowed_tool_permissions(
    parent_tool_permissions: Option<&AgentToolPermissions>,
    user_tool_defaults: &UserToolDefaults,
    allowed_tools: &[String],
) -> AgentToolPermissions {
    let mut per_tool = BTreeMap::new();
    for tool in allowed_tools {
        let parent_state =
            resolve_effective_permission(user_tool_defaults, parent_tool_permissions, tool);
        per_tool.insert(tool.clone(), parent_state);
    }
    AgentToolPermissions { per_tool }
}

/// Kernel policy for a `task` child: tools outside its allowlist are
/// denied (or asked about, when the user's default mode is auto-review).
pub(super) fn task_policy(
    permissions: AgentPermissions,
    tool_permissions: AgentToolPermissions,
    user_tool_defaults: &UserToolDefaults,
    mode: AgentMode,
) -> PolicyConfig {
    let fallback = match user_tool_defaults.mode {
        UserDefaultMode::AutoReview => ToolState::Ask,
        _ => ToolState::Deny,
    };
    let user_default = UserToolDefaults::default_permissions(BTreeMap::new(), fallback);
    PolicyConfig::default()
        .with_agent_permissions(permissions)
        .with_user_default(user_default)
        .with_agent_override(Some(tool_permissions))
        .with_mode(mode)
}

/// Kernel policy for a `spawn_agent` child: the parent's user defaults
/// with the child's own per-tool overrides on top.
pub(super) fn agent_policy(
    permissions: AgentPermissions,
    tool_permissions: Option<AgentToolPermissions>,
    user_tool_defaults: &UserToolDefaults,
    mode: AgentMode,
) -> PolicyConfig {
    PolicyConfig::default()
        .with_agent_permissions(permissions)
        .with_user_default(user_tool_defaults.clone())
        .with_agent_override(tool_permissions)
        .with_mode(mode)
}

/// Loop config for a child. The parent's identity carries the router and
/// billing envelope (`X-Aura-*`, auth token); without it, children of
/// unregistered parents run on a bare config.
pub(super) fn loop_config_for(
    model: &str,
    system_prompt: &str,
    identity: Option<AgentIdentity>,
) -> AgentLoopConfig {
    match identity {
        Some(identity) => identity.into_loop_config(),
        None => AgentLoopConfig {
            system_prompt: system_prompt.to_string(),
            ..AgentLoopConfig::for_agent(model)
        },
    }
}

/// Apply a `task` kind's budget on top of a loop config.
pub(super) fn apply_kind_budget(config: &mut AgentLoopConfig, kind: &SubagentKindSpec) {
    config.max_iterations = kind.budget.max_iterations as usize;
    if let Some(max_tokens) = kind.budget.max_tokens {
        config.max_tokens = config.max_tokens.min(max_tokens);
    }
}

pub(super) fn kind_system_prompt(kind: &SubagentKindSpec, addendum: Option<&str>) -> String {
    match addendum {
        Some(addendum) if !addendum.trim().is_empty() => {
            format!("{}\n\n{}", kind.system_prompt, addendum)
        }
        _ => kind.system_prompt.clone(),
    }
}

/// System prompt for a `spawn_agent` child: its base prompt plus who it
/// is and how its messages flow.
pub(super) fn spawned_agent_prompt(base: &str, name: &str, role: &str) -> String {
    let about = format!(
        "You are {name}, an agent with the role: {role}. Another agent started you and \
         sends you messages. Your final answer to each message is delivered back to its \
         sender. You can start agents of your own with spawn_agent or task, and message \
         other agents with send_to_agent."
    );
    if base.trim().is_empty() {
        about
    } else {
        format!("{base}\n\n{about}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_agent_subagent::{narrow_permissions, SubagentRegistry};
    use aura_core_types::{AgentScope, Capability};

    #[test]
    fn task_policy_denies_tools_outside_allowlist() {
        let user = UserToolDefaults::full_access();
        let registry = SubagentRegistry::bundled();
        let kind = registry.get("explore").unwrap();
        let tool_permissions = narrowed_tool_permissions(None, &user, &kind.allowed_tools);
        let policy = task_policy(
            AgentPermissions::empty(),
            tool_permissions,
            &user,
            AgentMode::Agent,
        );
        assert_eq!(
            resolve_effective_permission(
                &policy.user_default,
                policy.agent_override.as_ref(),
                "write_file",
            ),
            ToolState::Deny
        );
        assert_eq!(
            resolve_effective_permission(
                &policy.user_default,
                policy.agent_override.as_ref(),
                "read_file",
            ),
            ToolState::Allow
        );
    }

    #[test]
    fn explicit_empty_tool_subset_denies_every_tool() {
        let user = UserToolDefaults::full_access();
        let tool_permissions = narrowed_tool_permissions(None, &user, &[]);
        let policy = task_policy(
            AgentPermissions::empty(),
            tool_permissions,
            &user,
            AgentMode::Agent,
        );
        for tool in ["read_file", "run_command"] {
            assert_eq!(
                resolve_effective_permission(
                    &policy.user_default,
                    policy.agent_override.as_ref(),
                    tool,
                ),
                ToolState::Deny
            );
        }
    }

    fn parent_identity(model: &str) -> AgentIdentity {
        AgentIdentity {
            reasoning_effort: Some(aura_model_reasoner::ThinkingEffort::XHigh),
            upstream_provider_family: None,
            model: model.to_string(),
            aura_org_id: Some("org-parent".to_string()),
            aura_session_id: Some("session-parent".to_string()),
            aura_agent_id: Some("agent-parent".to_string()),
            aura_project_id: Some("project-parent".to_string()),
            system_prompt: "parent prompt".to_string(),
            prompt_cache_key: None,
            prompt_cache_retention: None,
            request_kind: aura_model_reasoner::ModelRequestKind::Chat,
            max_tokens: 1024,
            max_context_tokens: 200_000,
            auth_token: Some("parent-jwt".to_string()),
        }
    }

    #[test]
    fn loop_config_inherits_parent_identity_envelope() {
        let registry = SubagentRegistry::bundled();
        let kind = registry.get("explore").unwrap();
        let mut config = loop_config_for(
            "claude-opus-4-7",
            "child prompt",
            Some(parent_identity("claude-opus-4-7")),
        );
        apply_kind_budget(&mut config, kind);

        assert_eq!(config.aura_org_id.as_deref(), Some("org-parent"));
        assert_eq!(config.aura_session_id.as_deref(), Some("session-parent"));
        assert_eq!(config.aura_agent_id.as_deref(), Some("agent-parent"));
        assert_eq!(config.aura_project_id.as_deref(), Some("project-parent"));
        assert_eq!(config.auth_token.as_deref(), Some("parent-jwt"));
        assert_eq!(
            config.user_thinking_effort,
            Some(aura_model_reasoner::ThinkingEffort::XHigh)
        );
        assert_eq!(config.max_iterations, kind.budget.max_iterations as usize);
    }

    #[test]
    fn loop_config_without_identity_falls_back_to_bare_config() {
        let config = loop_config_for("claude-opus-4-7", "child prompt", None);
        assert!(config.aura_org_id.is_none());
        assert!(config.auth_token.is_none());
        assert_eq!(config.system_prompt, "child prompt");
    }

    #[test]
    fn narrow_permissions_keeps_only_kind_allowed_caps() {
        let parent = AgentPermissions {
            scope: AgentScope::default(),
            capabilities: vec![Capability::SpawnAgent, Capability::ReadAgent],
        };
        let mut kind = SubagentRegistry::bundled()
            .get("general_purpose")
            .unwrap()
            .clone();
        kind.allowed_capabilities = vec![Capability::ReadAgent];
        let narrowed = narrow_permissions(&parent, &kind);
        assert_eq!(narrowed.capabilities, vec![Capability::ReadAgent]);
    }

    #[test]
    fn general_purpose_children_keep_spawn() {
        let kind = SubagentRegistry::bundled()
            .get("general_purpose")
            .unwrap()
            .clone();
        let narrowed = narrow_permissions(&AgentPermissions::full_access(), &kind);
        assert!(narrowed.capabilities.contains(&Capability::SpawnAgent));
        assert!(kind.allowed_tools.iter().any(|t| t == "task"));
        assert!(kind.allowed_tools.iter().any(|t| t == "spawn_agent"));
    }
}
