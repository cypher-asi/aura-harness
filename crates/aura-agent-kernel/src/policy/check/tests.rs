//! Unit tests for the policy authorization pipeline.

use super::*;
use crate::policy::PolicyConfig;
use crate::ToolApprovalRemember;
use aura_core_types::{
    ActionKind, AgentPermissions, AgentScope, Capability, Proposal, ToolCall, ToolState,
};
use bytes::Bytes;

fn delegate_proposal(tool: &str, args: serde_json::Value) -> Proposal {
    let call = ToolCall::new(tool, args);
    let payload = serde_json::to_vec(&call).unwrap();
    Proposal::new(ActionKind::Delegate, Bytes::from(payload))
}

#[test]
fn default_permissions_allow_capability_gated_tools() {
    let policy = Policy::with_defaults();
    let proposal = delegate_proposal("run_command", serde_json::json!({"program":"git"}));
    assert!(policy.check(&proposal).allowed);
}

#[test]
fn explicit_empty_permissions_allow_unrestricted_tools() {
    // A tool that carries no capability requirement and no scope
    // target keys passes even against an explicit empty bundle.
    let policy =
        Policy::new(PolicyConfig::default().with_agent_permissions(AgentPermissions::empty()));
    let proposal = delegate_proposal("read_file", serde_json::json!({"path":"a.txt"}));
    assert!(policy.check(&proposal).allowed);
}

#[test]
fn missing_capability_is_denied() {
    let config = PolicyConfig::default()
        .with_agent_permissions(AgentPermissions {
            scope: AgentScope::default(),
            capabilities: vec![],
        })
        .with_tool_capability("spawn_agent", Capability::SpawnAgent);
    let policy = Policy::new(config);
    let result = policy.check(&delegate_proposal("spawn_agent", serde_json::json!({})));
    assert!(!result.allowed);
    assert!(result.reason.unwrap().contains("capability"));
}

#[test]
fn present_capability_is_allowed() {
    let config = PolicyConfig::default()
        .with_agent_permissions(AgentPermissions {
            scope: AgentScope::default(),
            capabilities: vec![Capability::SpawnAgent],
        })
        .with_tool_capability("spawn_agent", Capability::SpawnAgent);
    let policy = Policy::new(config);
    assert!(
        policy
            .check(&delegate_proposal("spawn_agent", serde_json::json!({})))
            .allowed
    );
}

#[test]
fn out_of_scope_target_is_denied() {
    let config = PolicyConfig::default().with_agent_permissions(AgentPermissions {
        scope: AgentScope {
            orgs: vec!["only".into()],
            ..AgentScope::default()
        },
        capabilities: vec![],
    });
    let policy = Policy::new(config);
    let result = policy.check(&delegate_proposal(
        "any_tool",
        serde_json::json!({"target_org_id":"other"}),
    ));
    assert!(!result.allowed);
    assert!(result.reason.unwrap().contains("out of scope"));
}

#[test]
fn resolve_tool_state_default_is_on() {
    let policy = Policy::with_defaults();
    assert_eq!(policy.resolve_tool_state("read_file"), ToolState::Allow);
    assert_eq!(policy.resolve_tool_state("run_command"), ToolState::Allow);
    assert_eq!(policy.resolve_tool_state("anything"), ToolState::Allow);
}

#[test]
fn resolve_tool_state_auto_review_is_ask_for_everything() {
    let cfg =
        PolicyConfig::default().with_user_default(aura_core_types::UserToolDefaults::auto_review());
    let policy = Policy::new(cfg);
    assert_eq!(policy.resolve_tool_state("read_file"), ToolState::Ask);
    assert_eq!(policy.resolve_tool_state("run_command"), ToolState::Ask);
}

#[test]
fn resolve_tool_state_default_permissions_mode_is_tri_state_per_tool() {
    let mut per_tool = std::collections::BTreeMap::new();
    per_tool.insert("read_file".into(), ToolState::Allow);
    per_tool.insert("run_command".into(), ToolState::Ask);
    per_tool.insert("delete_file".into(), ToolState::Deny);
    let user_default =
        aura_core_types::UserToolDefaults::default_permissions(per_tool, ToolState::Deny);
    let cfg = PolicyConfig::default().with_user_default(user_default);
    let policy = Policy::new(cfg);
    assert_eq!(policy.resolve_tool_state("read_file"), ToolState::Allow);
    assert_eq!(policy.resolve_tool_state("run_command"), ToolState::Ask);
    assert_eq!(policy.resolve_tool_state("delete_file"), ToolState::Deny);
    assert_eq!(
        policy.resolve_tool_state("not_in_map"),
        ToolState::Deny,
        "fallback applies to unlisted tools",
    );
}

#[test]
fn resolve_tool_state_agent_override_wins_over_user_default() {
    let cfg = PolicyConfig::default()
        .with_user_default(aura_core_types::UserToolDefaults::full_access())
        .with_agent_override(Some(
            aura_core_types::AgentToolPermissions::new()
                .with("run_command", ToolState::Deny)
                .with("delete_file", ToolState::Ask),
        ));
    let policy = Policy::new(cfg);
    assert_eq!(
        policy.resolve_tool_state("run_command"),
        ToolState::Deny,
        "override flips user's full_access to off",
    );
    assert_eq!(
        policy.resolve_tool_state("delete_file"),
        ToolState::Ask,
        "override flips user's full_access to ask",
    );
    assert_eq!(
        policy.resolve_tool_state("read_file"),
        ToolState::Allow,
        "unlisted tool still flows through user default (on)",
    );
}

#[test]
fn live_prompt_verdict_denies_ask_without_session() {
    let cfg =
        PolicyConfig::default().with_user_default(aura_core_types::UserToolDefaults::auto_review());
    let policy = Policy::new(cfg);
    let verdict = policy
        .live_tool_prompt_verdict(
            "read_file",
            &serde_json::json!({"path": "a.txt"}),
            aura_core_types::AgentId::generate(),
            "request-1".to_string(),
            false,
            vec![ToolApprovalRemember::Once],
        )
        .expect("ask state should produce a verdict");

    assert!(
        matches!(verdict, PolicyVerdict::Deny { ref reason } if reason.contains("no session to prompt"))
    );
}

#[test]
fn live_prompt_verdict_carries_structured_prompt() {
    let cfg =
        PolicyConfig::default().with_user_default(aura_core_types::UserToolDefaults::auto_review());
    let policy = Policy::new(cfg);
    let agent_id = aura_core_types::AgentId::generate();
    let verdict = policy
        .live_tool_prompt_verdict(
            "read_file",
            &serde_json::json!({"path": "a.txt"}),
            agent_id,
            "request-1".to_string(),
            true,
            vec![ToolApprovalRemember::Once, ToolApprovalRemember::Session],
        )
        .expect("ask state should produce a verdict");

    match verdict {
        PolicyVerdict::RequireApproval {
            prompt: Some(prompt),
            ..
        } => {
            assert_eq!(prompt.request_id, "request-1");
            assert_eq!(prompt.tool_name, "read_file");
            assert_eq!(prompt.args, serde_json::json!({"path": "a.txt"}));
            assert_eq!(prompt.agent_id, agent_id);
            assert_eq!(prompt.remember_options.len(), 2);
        }
        other => panic!("expected structured prompt, got {other:?}"),
    }
}

#[test]
fn in_scope_target_is_allowed() {
    let config = PolicyConfig::default().with_agent_permissions(AgentPermissions {
        scope: AgentScope {
            orgs: vec!["only".into()],
            ..AgentScope::default()
        },
        capabilities: vec![],
    });
    let policy = Policy::new(config);
    assert!(
        policy
            .check(&delegate_proposal(
                "any_tool",
                serde_json::json!({"target_org_id":"only"})
            ))
            .allowed
    );
}

fn mode_policy(mode: aura_core_modes::AgentMode) -> Policy {
    Policy::new(PolicyConfig::default().with_mode(mode))
}

fn mode_allows(mode: aura_core_modes::AgentMode, tool: &str, args: serde_json::Value) -> bool {
    mode_policy(mode)
        .check(&delegate_proposal(tool, args))
        .allowed
}

#[test]
fn mode_matrix_matches_the_gate_table() {
    use aura_core_modes::AgentMode::{Agent, Ask, Debug, Plan};

    let rs = || serde_json::json!({"path":"src/lib.rs","content":"x"});
    let md = || serde_json::json!({"path":"docs/PLAN.md","content":"x"});
    let cmd = || serde_json::json!({"program":"ls"});
    let none = || serde_json::json!({});

    // (tool, args, [Agent, Plan, Ask, Debug])
    let cases: Vec<(&str, serde_json::Value, [bool; 4])> = vec![
        (
            "read_file",
            serde_json::json!({"path":"a.rs"}),
            [true, true, true, true],
        ),
        ("write_file", rs(), [true, false, false, false]),
        ("write_file", md(), [true, true, false, false]),
        ("edit_file", rs(), [true, false, false, false]),
        ("delete_file", md(), [true, true, false, false]),
        ("create_spec", none(), [true, true, false, false]),
        ("git_commit", none(), [true, false, false, false]),
        ("run_command", cmd(), [true, true, false, false]),
        ("git_push", none(), [true, false, false, false]),
        ("create_task", none(), [true, false, false, false]),
        ("spawn_agent", none(), [true, false, false, false]),
        ("task", none(), [true, false, false, false]),
        ("list_specs", none(), [true, true, true, false]),
        ("unclassified_mcp_tool", none(), [true, false, false, false]),
    ];

    for (tool, args, expected) in cases {
        for (mode, want) in [Agent, Plan, Ask, Debug].into_iter().zip(expected) {
            assert_eq!(
                mode_allows(mode, tool, args.clone()),
                want,
                "{tool} {args} in {mode:?}"
            );
        }
    }
}

#[test]
fn mode_denial_names_the_mode_and_violation() {
    let result = mode_policy(aura_core_modes::AgentMode::Plan).check(&delegate_proposal(
        "write_file",
        serde_json::json!({"path":"src/main.rs","content":"x"}),
    ));
    assert!(!result.allowed);
    let reason = result.reason.unwrap();
    assert!(reason.contains("Plan mode"), "{reason}");
    assert!(reason.contains("non-markdown"), "{reason}");
}

#[test]
fn mode_gate_runs_before_tool_state() {
    // An explicit per-tool "on" cannot widen past the mode ceiling.
    let config = PolicyConfig::default()
        .with_mode(aura_core_modes::AgentMode::Ask)
        .with_agent_override(Some(
            aura_core_types::AgentToolPermissions::default().with("run_command", ToolState::Allow),
        ));
    let policy = Policy::new(config);
    let result = policy.check(&delegate_proposal(
        "run_command",
        serde_json::json!({"program":"ls"}),
    ));
    assert!(!result.allowed);
}
