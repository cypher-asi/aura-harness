use super::*;
use bytes::Bytes;

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
        PermissionLevel::AlwaysAllow
    );
    assert_eq!(
        default_tool_permission("unknown_tool"),
        PermissionLevel::Deny
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
fn test_default_policy_allows_unlisted_tool() {
    let policy = Policy::with_defaults();
    let tool_call = ToolCall::new("unknown.tool", serde_json::json!({}));
    let payload = serde_json::to_vec(&tool_call).unwrap();
    let proposal = Proposal::new(ActionKind::Delegate, Bytes::from(payload));

    let result = policy.check(&proposal);
    assert!(result.allowed);
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
    assert!(!policy.requires_approval("run_command"));
}

#[test]
fn test_unlisted_tool_allowed_by_default() {
    let policy = Policy::with_defaults();
    assert!(!policy.requires_approval("some_unknown_tool"));
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
fn test_check_tool_unlisted_allowed_by_default() {
    let policy = Policy::with_defaults();
    let result = policy.check_tool("evil_tool", &serde_json::json!({}));
    assert!(result.allowed);
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
fn test_always_ask_permission_override() {
    let config =
        PolicyConfig::default().with_tool_permission("read_file", PermissionLevel::AlwaysAsk);
    let policy = Policy::new(config);

    let result = policy.check_tool("read_file", &serde_json::json!({}));
    assert!(!result.allowed);
    assert!(result
        .reason
        .unwrap()
        .contains("requires approval for each use"));
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
fn test_check_denies_always_ask_tool() {
    let config =
        PolicyConfig::default().with_tool_permission("read_file", PermissionLevel::AlwaysAsk);
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
}
