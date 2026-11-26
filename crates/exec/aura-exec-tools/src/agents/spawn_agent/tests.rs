use super::*;
use crate::sandbox::Sandbox;
use crate::ToolConfig;
use async_trait::async_trait;
use aura_core_types::{AgentScope, Capability, Hash};
use aura_exec_traits::{SpawnError, SpawnHook, SpawnOutcome};
use std::sync::{Arc, Mutex};

/// Test-only spawn hook that records each invocation so assertions can
/// inspect the parent id + originating user id that the tool forwarded.
#[derive(Default)]
pub(crate) struct MockSpawnHook {
    pub calls: Mutex<Vec<(AgentId, Option<String>, aura_exec_traits::ChildAgentSpec)>>,
}

#[async_trait]
impl SpawnHook for MockSpawnHook {
    async fn spawn_child(
        &self,
        parent_agent_id: &AgentId,
        originating_user_id: Option<&str>,
        child: aura_exec_traits::ChildAgentSpec,
    ) -> Result<SpawnOutcome, SpawnError> {
        let child_agent_id = child.preassigned_agent_id.unwrap_or_else(AgentId::generate);
        self.calls.lock().unwrap().push((
            *parent_agent_id,
            originating_user_id.map(ToString::to_string),
            child,
        ));
        Ok(SpawnOutcome {
            child_agent_id,
            external_agent_id: None,
            delegate_tx_hash: Hash::default(),
        })
    }
}

fn ctx_with(
    caller: AgentPermissions,
    caller_agent_id: Option<AgentId>,
    parent_chain: Vec<AgentId>,
) -> ToolContext {
    let dir = std::env::temp_dir();
    let mut ctx = ToolContext::new(Sandbox::new(&dir).unwrap(), ToolConfig::default());
    ctx.caller_permissions = Some(caller);
    ctx.caller_agent_id = caller_agent_id;
    ctx.parent_chain = parent_chain;
    ctx.originating_user_id = Some("user-root".into());
    ctx
}

pub(crate) fn ctx_with_hook(
    caller: AgentPermissions,
    caller_agent_id: Option<AgentId>,
    hook: Arc<MockSpawnHook>,
) -> ToolContext {
    let mut ctx = ctx_with(caller, caller_agent_id, vec![]);
    ctx.spawn_hook = Some(hook);
    ctx
}

fn ceo() -> AgentPermissions {
    AgentPermissions::ceo_preset()
}

#[test]
fn spawn_agent_denies_scope_escalation() {
    let caller = AgentPermissions {
        scope: AgentScope {
            orgs: vec!["a".into()],
            ..AgentScope::default()
        },
        capabilities: vec![Capability::SpawnAgent],
    };
    let ctx = ctx_with(caller, Some(AgentId::generate()), vec![]);

    let input = SpawnAgentInput {
        name: "child".into(),
        role: "worker".into(),
        permissions: Some(AgentPermissions {
            scope: AgentScope {
                orgs: vec!["a".into(), "b".into()],
                ..AgentScope::default()
            },
            capabilities: vec![Capability::SpawnAgent],
        }),
        agent_id: None,
        system_prompt: None,
        tool_permissions: None,
        prompt: None,
    };

    let err = SpawnAgentTool::evaluate(&ctx, &input).unwrap_err();
    assert!(err.to_string().contains("strict subset"), "got: {err}");
}

#[test]
fn spawn_agent_denies_capability_escalation() {
    let caller = AgentPermissions {
        scope: AgentScope::default(),
        capabilities: vec![Capability::SpawnAgent],
    };
    let ctx = ctx_with(caller, Some(AgentId::generate()), vec![]);

    let input = SpawnAgentInput {
        name: "child".into(),
        role: "worker".into(),
        permissions: Some(AgentPermissions {
            scope: AgentScope::default(),
            capabilities: vec![Capability::SpawnAgent, Capability::ManageBilling],
        }),
        agent_id: None,
        system_prompt: None,
        tool_permissions: None,
        prompt: None,
    };

    let err = SpawnAgentTool::evaluate(&ctx, &input).unwrap_err();
    assert!(err.to_string().contains("strict subset"), "got: {err}");
}

#[test]
fn spawn_agent_rejects_ancestor_cycle() {
    let ancestor = AgentId::generate();
    let ctx = ctx_with(ceo(), Some(AgentId::generate()), vec![ancestor]);

    let input = SpawnAgentInput {
        name: "child".into(),
        role: "worker".into(),
        permissions: Some(AgentPermissions::empty()),
        agent_id: Some(ancestor.to_string()),
        system_prompt: None,
        tool_permissions: None,
        prompt: None,
    };

    let err = SpawnAgentTool::evaluate(&ctx, &input).unwrap_err();
    assert!(err.to_string().contains("cycle"), "got: {err}");
}

#[test]
fn spawn_agent_allows_proper_subset() {
    let caller_id = AgentId::generate();
    let ctx = ctx_with(ceo(), Some(caller_id), vec![]);

    let input = SpawnAgentInput {
        name: "child".into(),
        role: "worker".into(),
        permissions: Some(AgentPermissions {
            scope: AgentScope::default(),
            capabilities: vec![Capability::SpawnAgent],
        }),
        agent_id: None,
        system_prompt: None,
        tool_permissions: None,
        prompt: None,
    };

    let outcome = SpawnAgentTool::evaluate(&ctx, &input).unwrap();
    assert_eq!(outcome.name, "child");
    assert_eq!(outcome.role, "worker");
    assert_eq!(outcome.parent_agent_id, Some(caller_id.to_string()));
    assert_eq!(outcome.originating_user_id.as_deref(), Some("user-root"));
    assert!(!outcome.child_agent_id.is_empty());
}

#[test]
fn spawn_agent_requires_caller_permissions_on_ctx() {
    let dir = std::env::temp_dir();
    let ctx = ToolContext::new(Sandbox::new(&dir).unwrap(), ToolConfig::default());
    let input = SpawnAgentInput {
        name: "x".into(),
        role: "x".into(),
        permissions: Some(AgentPermissions::empty()),
        agent_id: None,
        system_prompt: None,
        tool_permissions: None,
        prompt: None,
    };
    let err = SpawnAgentTool::evaluate(&ctx, &input).unwrap_err();
    assert!(err.to_string().contains("caller_permissions"), "got: {err}");
}

#[tokio::test]
async fn spawn_agent_tool_wraps_outcome_in_tool_result() {
    let caller_id = AgentId::generate();
    let ctx = ctx_with(ceo(), Some(caller_id), vec![]);
    let args = serde_json::json!({
        "name": "child",
        "role": "worker",
        "permissions": AgentPermissions::empty()
    });
    let result = SpawnAgentTool.execute(&ctx, args).await.unwrap();
    assert!(result.ok);
    let outcome: SpawnAgentOutcome = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(outcome.parent_agent_id, Some(caller_id.to_string()));
}

#[tokio::test]
async fn spawn_agent_invokes_hook_with_parent_and_user_id() {
    let caller_id = AgentId::generate();
    let hook = Arc::new(MockSpawnHook::default());
    let ctx = ctx_with_hook(ceo(), Some(caller_id), Arc::clone(&hook));

    let args = serde_json::json!({
        "name": "child",
        "role": "worker",
        "permissions": AgentPermissions::empty(),
    });
    let result = SpawnAgentTool.execute(&ctx, args).await.unwrap();
    assert!(
        result.ok,
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );

    let calls = hook.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let (parent, user, spec) = &calls[0];
    assert_eq!(*parent, caller_id);
    assert_eq!(user.as_deref(), Some("user-root"));
    assert_eq!(spec.name, "child");
    assert_eq!(spec.role, "worker");
}
