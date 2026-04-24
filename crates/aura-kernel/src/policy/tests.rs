use super::*;
use aura_core::{
    ActionKind, InstalledIntegrationDefinition, InstalledToolCapability,
    InstalledToolIntegrationRequirement, Proposal, RuntimeCapabilityInstall, SystemKind, ToolCall,
};
use bytes::Bytes;
use std::collections::{HashMap, HashSet};

#[test]
fn test_default_permissions() {
    assert_eq!(
        default_tool_permission("read_file"),
        PermissionLevel::AlwaysAllow
    );
    assert_eq!(
        default_tool_permission("write_file"),
        PermissionLevel::AlwaysAllow
    );
    assert_eq!(
        default_tool_permission("run_command"),
        PermissionLevel::RequireApproval
    );
    assert_eq!(
        default_tool_permission("unknown_tool"),
        PermissionLevel::Deny
    );
}

#[test]
fn test_always_ask_serde_alias_deserializes_to_require_approval() {
    // Phase 6 rename: old configs serialized with `"always_ask"` must
    // continue to parse into the new `RequireApproval` variant via the
    // `#[serde(alias = "always_ask")]` on the enum.
    let legacy: PermissionLevel =
        serde_json::from_str("\"always_ask\"").expect("legacy alias should deserialize");
    assert_eq!(legacy, PermissionLevel::RequireApproval);

    // Forward direction: the canonical serde tag for the new variant
    // is `"require_approval"` (snake_case of `RequireApproval`).
    let canonical: PermissionLevel =
        serde_json::from_str("\"require_approval\"").expect("canonical tag should deserialize");
    assert_eq!(canonical, PermissionLevel::RequireApproval);
    assert_eq!(
        serde_json::to_string(&PermissionLevel::RequireApproval).unwrap(),
        "\"require_approval\""
    );
}

#[test]
fn test_policy_allows_reason() {
    let policy = Policy::with_defaults();
    let proposal = Proposal::new(ActionKind::Reason, Bytes::new());

    let result = policy.check(&proposal);
    assert!(result.allowed);
}

#[test]
fn test_policy_allows_fs_read() {
    let policy = Policy::with_defaults();
    let tool_call = ToolCall::fs_read("test.txt", None);
    let payload = serde_json::to_vec(&tool_call).unwrap();
    let proposal = Proposal::new(ActionKind::Delegate, Bytes::from(payload));

    let result = policy.check(&proposal);
    assert!(result.allowed);
}

#[test]
fn test_default_policy_denies_unlisted_tool() {
    // Phase 5 hardening: defaults are fail-closed. Unlisted tools
    // fall through to `Deny` instead of `AlwaysAllow`.
    let policy = Policy::with_defaults();
    let tool_call = ToolCall::new("unknown.tool", serde_json::json!({}));
    let payload = serde_json::to_vec(&tool_call).unwrap();
    let proposal = Proposal::new(ActionKind::Delegate, Bytes::from(payload));

    let result = policy.check(&proposal);
    assert!(!result.allowed);
    assert!(result.reason.unwrap().contains("not allowed"));
}

#[test]
fn test_explicit_allow_unlisted_allows_unknown_tool() {
    // Opt-in: hosts that genuinely want the pre-phase-5 wide-open
    // behavior must now set `allow_unlisted: true` explicitly.
    let config = PolicyConfig {
        allow_unlisted: true,
        ..PolicyConfig::default()
    };
    let policy = Policy::new(config);
    let tool_call = ToolCall::new("unknown.tool", serde_json::json!({}));
    let payload = serde_json::to_vec(&tool_call).unwrap();
    let proposal = Proposal::new(ActionKind::Delegate, Bytes::from(payload));

    assert!(policy.check(&proposal).allowed);
}

#[test]
fn test_default_config_fail_closed_flags() {
    // Phase 5 hardening: nail the exact shape of the default policy so
    // anyone loosening these knobs is forced to update the tests too.
    let cfg = PolicyConfig::default();
    assert!(
        !cfg.allow_unlisted,
        "default PolicyConfig must have allow_unlisted=false"
    );
    assert!(
        !cfg.allowed_tools.contains("run_command"),
        "default allowed_tools must NOT contain run_command"
    );
    // Read-only / narrow filesystem tools stay in.
    assert!(cfg.allowed_tools.contains("read_file"));
    assert!(cfg.allowed_tools.contains("write_file"));
    // `default_tool_permission` still classes run_command as
    // `RequireApproval` (renamed from `AlwaysAsk` in Phase 6) so hosts
    // that opt in get per-invocation approval.
    assert_eq!(
        default_tool_permission("run_command"),
        PermissionLevel::RequireApproval
    );
}

#[test]
fn test_default_config_denies_run_command_delegate() {
    // End-to-end: `run_command` is not in the default allow-list and
    // `allow_unlisted` is false, so a Delegate proposal that targets
    // `run_command` is rejected at policy check.
    let policy = Policy::with_defaults();
    let tool_call = ToolCall::new("run_command", serde_json::json!({"program": "ls"}));
    let payload = serde_json::to_vec(&tool_call).unwrap();
    let proposal = Proposal::new(ActionKind::Delegate, Bytes::from(payload));

    let result = policy.check(&proposal);
    assert!(!result.allowed);
    assert!(result.reason.unwrap().contains("not allowed"));
}

#[test]
fn test_restrictive_policy_blocks_unknown_tool() {
    let policy = Policy::new(PolicyConfig::restrictive());
    let tool_call = ToolCall::new("unknown.tool", serde_json::json!({}));
    let payload = serde_json::to_vec(&tool_call).unwrap();
    let proposal = Proposal::new(ActionKind::Delegate, Bytes::from(payload));

    let result = policy.check(&proposal);
    assert!(!result.allowed);
}

#[test]
fn test_session_approvals() {
    let config =
        PolicyConfig::default().with_tool_permission("guarded_tool", PermissionLevel::AskOnce);
    let policy = Policy::new(config);

    assert!(policy.requires_approval("guarded_tool"));

    policy.approve_for_session("guarded_tool");

    assert!(!policy.requires_approval("guarded_tool"));
    assert!(policy.is_session_approved("guarded_tool"));

    policy.clear_session_approvals();
    assert!(policy.requires_approval("guarded_tool"));
}

#[test]
fn test_permission_override() {
    let config =
        PolicyConfig::default().with_tool_permission("read_file", PermissionLevel::AskOnce);

    let policy = Policy::new(config);

    assert_eq!(
        policy.check_tool_permission("read_file"),
        PermissionLevel::AskOnce
    );
}

#[test]
fn test_restrictive_config() {
    let config = PolicyConfig::restrictive();
    let policy = Policy::new(config);

    assert_eq!(
        policy.check_tool_permission("read_file"),
        PermissionLevel::AlwaysAllow
    );

    assert_eq!(
        policy.check_tool_permission("write_file"),
        PermissionLevel::Deny
    );
}

#[test]
fn test_revoke_session_approval() {
    let config =
        PolicyConfig::default().with_tool_permission("guarded_tool", PermissionLevel::AskOnce);
    let policy = Policy::new(config);

    policy.approve_for_session("guarded_tool");
    assert!(policy.is_session_approved("guarded_tool"));

    policy.revoke_session_approval("guarded_tool");
    assert!(!policy.is_session_approved("guarded_tool"));
    assert!(policy.requires_approval("guarded_tool"));
}

#[test]
fn test_clear_session_approvals_multiple() {
    let policy = Policy::with_defaults();

    policy.approve_for_session("write_file");
    policy.approve_for_session("edit_file");
    assert!(policy.is_session_approved("write_file"));
    assert!(policy.is_session_approved("edit_file"));

    policy.clear_session_approvals();
    assert!(!policy.is_session_approved("write_file"));
    assert!(!policy.is_session_approved("edit_file"));
}

#[test]
fn test_revoke_nonexistent_approval_is_noop() {
    let policy = Policy::with_defaults();
    policy.revoke_session_approval("write_file");
    assert!(!policy.is_session_approved("write_file"));
}

#[test]
fn test_always_allow_does_not_require_approval() {
    let policy = Policy::with_defaults();
    assert!(!policy.requires_approval("read_file"));
    assert!(!policy.requires_approval("list_files"));
    // `run_command` now defaults to `RequireApproval` (Wave 5 / T3.3,
    // renamed Phase 6); verify the read-only tools above still pass
    // through without approval.
    assert!(policy.requires_approval("run_command"));
}

#[test]
fn test_unlisted_tool_requires_approval_by_default() {
    // Phase 5 hardening: `allow_unlisted=false` means
    // `check_tool_permission` returns `Deny`, which `requires_approval`
    // translates to `true` (deny => caller must ask).
    let policy = Policy::with_defaults();
    assert!(policy.requires_approval("some_unknown_tool"));
}

#[test]
fn test_unlisted_tool_denied_in_restrictive() {
    let policy = Policy::new(PolicyConfig::restrictive());
    assert!(policy.requires_approval("some_unknown_tool"));
}

#[test]
fn test_check_tool_always_allow() {
    let policy = Policy::with_defaults();
    let result = policy.check_tool("read_file", &serde_json::json!({}));
    assert!(result.allowed);
    assert!(result.reason.is_none());
}

#[test]
fn test_check_tool_unlisted_denied_by_default() {
    // Phase 5 hardening: defaults now deny unlisted tools.
    let policy = Policy::with_defaults();
    let result = policy.check_tool("evil_tool", &serde_json::json!({}));
    assert!(!result.allowed);
    assert!(result.reason.unwrap().contains("not allowed"));
}

#[test]
fn test_check_tool_denied_in_restrictive() {
    let policy = Policy::new(PolicyConfig::restrictive());
    let result = policy.check_tool("evil_tool", &serde_json::json!({}));
    assert!(!result.allowed);
    assert!(result.reason.unwrap().contains("not allowed"));
}

#[test]
fn test_check_tool_denies_missing_required_integration() {
    let mut config = PolicyConfig::default();
    config.add_allowed_tool("brave_search_web");
    config.set_tool_integration_requirements([(
        "brave_search_web".to_string(),
        aura_core::InstalledToolIntegrationRequirement {
            integration_id: None,
            provider: Some("brave_search".to_string()),
            kind: Some("workspace_integration".to_string()),
        },
    )]);
    let policy = Policy::new(config);

    let result = policy.check_tool("brave_search_web", &serde_json::json!({}));
    assert!(!result.allowed);
    assert!(result
        .reason
        .unwrap()
        .contains("requires an installed integration"));
}

#[test]
fn test_check_tool_allows_installed_required_integration() {
    let mut config = PolicyConfig::default();
    config.add_allowed_tool("brave_search_web");
    config.set_installed_integrations([aura_core::InstalledIntegrationDefinition {
        integration_id: "integration-brave-1".to_string(),
        name: "Brave Search".to_string(),
        provider: "brave_search".to_string(),
        kind: "workspace_integration".to_string(),
        metadata: std::collections::HashMap::new(),
    }]);
    config.set_tool_integration_requirements([(
        "brave_search_web".to_string(),
        aura_core::InstalledToolIntegrationRequirement {
            integration_id: None,
            provider: Some("brave_search".to_string()),
            kind: Some("workspace_integration".to_string()),
        },
    )]);
    let policy = Policy::new(config);

    let result = policy.check_tool("brave_search_web", &serde_json::json!({}));
    assert!(result.allowed);
}

#[test]
fn test_check_tool_uses_runtime_capability_ledger_as_source_of_truth() {
    let mut config = PolicyConfig::default();
    config.add_allowed_tool("brave_search_web");
    config.set_installed_integrations([InstalledIntegrationDefinition {
        integration_id: "integration-brave-1".to_string(),
        name: "Brave Search".to_string(),
        provider: "brave_search".to_string(),
        kind: "workspace_integration".to_string(),
        metadata: HashMap::new(),
    }]);
    config.set_tool_integration_requirements([(
        "brave_search_web".to_string(),
        InstalledToolIntegrationRequirement {
            integration_id: None,
            provider: Some("brave_search".to_string()),
            kind: Some("workspace_integration".to_string()),
        },
    )]);
    let policy = Policy::new(config);
    let runtime_capabilities = RuntimeCapabilityInstall {
        system_kind: SystemKind::CapabilityInstall,
        scope: "session".to_string(),
        session_id: Some("session-1".to_string()),
        installed_integrations: vec![],
        installed_tools: vec![],
    };

    let result = policy.check_tool_with_runtime_capabilities(
        "brave_search_web",
        &serde_json::json!({}),
        Some(&runtime_capabilities),
    );
    assert!(!result.allowed);
    assert!(result
        .reason
        .unwrap()
        .contains("kernel runtime capability ledger"));
}

#[test]
fn test_check_tool_allows_when_runtime_capability_ledger_has_matching_install() {
    let mut config = PolicyConfig::default();
    config.add_allowed_tool("brave_search_web");
    config.set_tool_integration_requirements([(
        "brave_search_web".to_string(),
        InstalledToolIntegrationRequirement {
            integration_id: None,
            provider: Some("brave_search".to_string()),
            kind: Some("workspace_integration".to_string()),
        },
    )]);
    let policy = Policy::new(config);
    let runtime_capabilities = RuntimeCapabilityInstall {
        system_kind: SystemKind::CapabilityInstall,
        scope: "session".to_string(),
        session_id: Some("session-1".to_string()),
        installed_integrations: vec![InstalledIntegrationDefinition {
            integration_id: "integration-brave-1".to_string(),
            name: "Brave Search".to_string(),
            provider: "brave_search".to_string(),
            kind: "workspace_integration".to_string(),
            metadata: HashMap::new(),
        }],
        installed_tools: vec![InstalledToolCapability {
            name: "brave_search_web".to_string(),
            required_integration: Some(InstalledToolIntegrationRequirement {
                integration_id: None,
                provider: Some("brave_search".to_string()),
                kind: Some("workspace_integration".to_string()),
            }),
        }],
    };

    let result = policy.check_tool_with_runtime_capabilities(
        "brave_search_web",
        &serde_json::json!({}),
        Some(&runtime_capabilities),
    );
    assert!(result.allowed);
}

#[test]
fn test_check_tool_ask_once_not_approved() {
    let config =
        PolicyConfig::default().with_tool_permission("guarded_tool", PermissionLevel::AskOnce);
    let policy = Policy::new(config);
    let result = policy.check_tool("guarded_tool", &serde_json::json!({}));
    assert!(!result.allowed);
    assert!(result.reason.unwrap().contains("requires approval"));
}

#[test]
fn test_check_tool_ask_once_after_approval() {
    let config =
        PolicyConfig::default().with_tool_permission("guarded_tool", PermissionLevel::AskOnce);
    let policy = Policy::new(config);
    policy.approve_for_session("guarded_tool");
    let result = policy.check_tool("guarded_tool", &serde_json::json!({}));
    assert!(result.allowed);
}

#[test]
fn test_require_approval_permission_override() {
    let config =
        PolicyConfig::default().with_tool_permission("read_file", PermissionLevel::RequireApproval);
    let policy = Policy::new(config);

    let result = policy.check_tool("read_file", &serde_json::json!({}));
    assert!(!result.allowed);
    assert!(result
        .reason
        .unwrap()
        .contains("requires approval for each use"));
    // Phase 6 — the structured verdict must distinguish this from a
    // plain `Deny` so HTTP callers can return 423 Locked.
    assert!(matches!(
        result.verdict,
        PolicyVerdict::RequireApproval { .. }
    ));
}

#[test]
fn test_max_proposals() {
    let policy = Policy::with_defaults();
    assert_eq!(policy.max_proposals(), 8);
}

#[test]
fn test_permissive_config_includes_cmd_run() {
    let config = PolicyConfig::permissive();
    assert!(config.allowed_tools.contains("run_command"));
}

#[test]
fn test_concurrent_session_approvals() {
    use std::sync::Arc;
    let policy = Arc::new(Policy::with_defaults());

    let handles: Vec<_> = (0..10)
        .map(|i| {
            let p = Arc::clone(&policy);
            std::thread::spawn(move || {
                let tool = format!("tool_{i}");
                p.approve_for_session(&tool);
                assert!(p.is_session_approved(&tool));
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn test_malformed_delegate_payload_rejected() {
    let policy = Policy::with_defaults();
    let proposal = Proposal::new(ActionKind::Delegate, Bytes::from_static(b"not valid json"));

    let result = policy.check(&proposal);
    assert!(!result.allowed);
    assert!(result
        .reason
        .unwrap()
        .contains("Malformed delegate payload"));
}

#[test]
fn test_check_proposal_disallowed_action_kind() {
    let mut allowed = HashSet::new();
    allowed.insert(ActionKind::Reason);
    let config = PolicyConfig {
        allowed_action_kinds: allowed,
        ..PolicyConfig::default()
    };
    let policy = Policy::new(config);

    let proposal = Proposal::new(ActionKind::Delegate, Bytes::new());
    let result = policy.check(&proposal);
    assert!(!result.allowed);
    assert!(result.reason.unwrap().contains("not allowed"));
}

#[test]
fn test_add_allowed_tool() {
    let mut config = PolicyConfig::default();
    config.add_allowed_tool("custom_tool");

    let policy = Policy::new(config);
    assert_eq!(
        policy.check_tool_permission("custom_tool"),
        PermissionLevel::AlwaysAllow
    );
    assert!(!policy.requires_approval("custom_tool"));
}

#[test]
fn test_add_allowed_tools_batch() {
    let mut config = PolicyConfig::default();
    config.add_allowed_tools(vec!["tool_a", "tool_b", "tool_c"]);

    let policy = Policy::new(config);
    assert_eq!(
        policy.check_tool_permission("tool_a"),
        PermissionLevel::AlwaysAllow
    );
    assert_eq!(
        policy.check_tool_permission("tool_b"),
        PermissionLevel::AlwaysAllow
    );
    assert_eq!(
        policy.check_tool_permission("tool_c"),
        PermissionLevel::AlwaysAllow
    );
}

#[test]
fn test_policy_add_allowed_tools() {
    let mut policy = Policy::new(PolicyConfig::restrictive());
    assert!(policy.requires_approval("custom_installed_tool"));

    policy.add_allowed_tools(vec!["custom_installed_tool"]);
    assert!(!policy.requires_approval("custom_installed_tool"));
    assert_eq!(
        policy.check_tool_permission("custom_installed_tool"),
        PermissionLevel::AlwaysAllow
    );
}

#[test]
fn test_check_denies_ask_once_tool_without_approval() {
    let config =
        PolicyConfig::default().with_tool_permission("guarded_tool", PermissionLevel::AskOnce);
    let policy = Policy::new(config);
    let tool_call = ToolCall::new("guarded_tool", serde_json::json!({"path": "f.txt"}));
    let payload = serde_json::to_vec(&tool_call).unwrap();
    let proposal = Proposal::new(ActionKind::Delegate, Bytes::from(payload));

    let result = policy.check(&proposal);
    assert!(!result.allowed);
    assert!(result.reason.unwrap().contains("requires approval"));
}

#[test]
fn test_check_allows_ask_once_tool_after_approval() {
    let config =
        PolicyConfig::default().with_tool_permission("guarded_tool", PermissionLevel::AskOnce);
    let policy = Policy::new(config);
    policy.approve_for_session("guarded_tool");

    let tool_call = ToolCall::new("guarded_tool", serde_json::json!({"path": "f.txt"}));
    let payload = serde_json::to_vec(&tool_call).unwrap();
    let proposal = Proposal::new(ActionKind::Delegate, Bytes::from(payload));

    let result = policy.check(&proposal);
    assert!(result.allowed);
}

#[test]
fn test_check_denies_require_approval_tool() {
    let config =
        PolicyConfig::default().with_tool_permission("read_file", PermissionLevel::RequireApproval);
    let policy = Policy::new(config);

    let tool_call = ToolCall::new("read_file", serde_json::json!({"path": "f.txt"}));
    let payload = serde_json::to_vec(&tool_call).unwrap();
    let proposal = Proposal::new(ActionKind::Delegate, Bytes::from(payload));

    let result = policy.check(&proposal);
    assert!(!result.allowed);
    assert!(result
        .reason
        .unwrap()
        .contains("requires approval for each use"));
    assert!(matches!(
        result.verdict,
        PolicyVerdict::RequireApproval { .. }
    ));
}

/// `add_allowed_tools` forces `AlwaysAllow` (it's the "install"
/// semantic for operator-trusted tools), whereas `allow_tool_names`
/// must NOT touch `tool_permissions` — the effective level stays at
/// `default_tool_permission(name)`. This distinction is what lets the
/// dev-loop automaton allow-list `git_push` for LLM dispatch while
/// keeping it at `RequireApproval` until the operator supplies remote
/// credentials. A regression to "allow_tool_names elevates to
/// AlwaysAllow" would silently let autonomous dev-loops push to
/// whatever upstream the workspace happens to have configured.
#[test]
fn allow_tool_names_preserves_default_permission_unlike_add_allowed_tools() {
    let mut cfg_with_elevation = PolicyConfig::default();
    cfg_with_elevation.add_allowed_tools(["git_push"]);
    assert_eq!(
        cfg_with_elevation.tool_permissions.get("git_push"),
        Some(&PermissionLevel::AlwaysAllow),
        "add_allowed_tools must still force AlwaysAllow (install semantic)"
    );
    assert!(cfg_with_elevation.allowed_tools.contains("git_push"));

    let mut cfg_no_elevation = PolicyConfig::default();
    cfg_no_elevation.allow_tool_names(["git_push"]);
    assert!(
        cfg_no_elevation.allowed_tools.contains("git_push"),
        "allow_tool_names must add to allowed_tools set"
    );
    assert!(
        !cfg_no_elevation.tool_permissions.contains_key("git_push"),
        "allow_tool_names must NOT override tool_permissions"
    );

    let policy = Policy::new(cfg_no_elevation);
    assert_eq!(
        policy.check_tool_permission("git_push"),
        PermissionLevel::RequireApproval,
        "allow_tool_names must preserve default_tool_permission (RequireApproval for git_push)"
    );
}
