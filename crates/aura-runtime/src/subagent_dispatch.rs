//! Runtime implementation of foreground subagent dispatch.

use crate::scheduler::Scheduler;
use crate::subagent_registry::SubagentRegistry;
use async_trait::async_trait;
use aura_agent::AgentLoopConfig;
use aura_core::{
    resolve_effective_permission, AgentPermissions, AgentToolPermissions, SubagentDispatchRequest,
    SubagentExit, SubagentKindSpec, SubagentResult, ToolState, Transaction, TransactionType,
    UserDefaultMode, UserToolDefaults,
};
use aura_kernel::{ChildAgentSpec, KernelSpawnHook, PolicyConfig, SpawnHook};
use aura_store::Store;
use aura_tools::SubagentDispatchHook;
use bytes::Bytes;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Foreground `task` dispatcher backed by the local scheduler.
pub struct RuntimeSubagentDispatch {
    store: Arc<dyn Store>,
    scheduler: Arc<Scheduler>,
    registry: SubagentRegistry,
    spawn_hook: KernelSpawnHook,
    /// Serializes `KernelSpawnHook` parent delegate writes while the current
    /// parent kernel is still inside a batch tool execution.
    spawn_lock: Mutex<()>,
}

impl RuntimeSubagentDispatch {
    #[must_use]
    pub fn new(store: Arc<dyn Store>, scheduler: Arc<Scheduler>) -> Self {
        Self {
            spawn_hook: KernelSpawnHook::new(store.clone()),
            store,
            scheduler,
            registry: SubagentRegistry::bundled(),
            spawn_lock: Mutex::new(()),
        }
    }

    #[must_use]
    pub fn with_registry(mut self, registry: SubagentRegistry) -> Self {
        self.registry = registry;
        self
    }
}

#[async_trait]
impl SubagentDispatchHook for RuntimeSubagentDispatch {
    async fn dispatch(&self, request: SubagentDispatchRequest) -> Result<SubagentResult, String> {
        let Some(kind) = self.registry.get(&request.subagent_type).cloned() else {
            return Ok(SubagentResult::rejected(format!(
                "unknown subagent type '{}'",
                request.subagent_type
            )));
        };

        let child_permissions = narrow_permissions(&request.parent_permissions, &kind);
        let child_tool_permissions = narrowed_tool_permissions(&request, &kind);
        let child_spec = ChildAgentSpec {
            name: kind.name.clone(),
            role: format!("subagent:{}", kind.name),
            permissions: child_permissions.clone(),
            tool_permissions: Some(child_tool_permissions.clone()),
            parent_tool_permissions: request.parent_tool_permissions.clone(),
            system_prompt_override: Some(system_prompt_for(&kind, &request)),
            preassigned_agent_id: None,
        };

        let spawn_outcome = {
            let _guard = self.spawn_lock.lock().await;
            self.spawn_hook
                .spawn_child(
                    &request.parent_agent_id,
                    request.originating_user_id.as_deref(),
                    child_spec,
                )
                .await
                .map_err(|e| format!("spawn child: {e}"))?
        };
        let child_agent_id = spawn_outcome.child_agent_id;

        let tx = Transaction::new_chained(
            child_agent_id,
            TransactionType::UserPrompt,
            Bytes::from(request.prompt.clone().into_bytes()),
            None,
        );
        self.store
            .enqueue_tx(&tx)
            .map_err(|e| format!("enqueue child prompt: {e}"))?;

        let loop_config = loop_config_for(&kind);
        let policy = policy_for(child_permissions, child_tool_permissions, &request);
        let processed = self
            .scheduler
            .schedule_agent_with_overrides(child_agent_id, Some(loop_config), Some(policy))
            .await
            .map_err(|e| format!("schedule child: {e}"))?;

        let Some(result) = processed.last_result else {
            return Ok(SubagentResult {
                child_agent_id: Some(child_agent_id),
                final_message: String::new(),
                total_input_tokens: 0,
                total_output_tokens: 0,
                files_changed: Vec::new(),
                exit: SubagentExit::Failed {
                    reason: "child processed no agent loop result".into(),
                },
            });
        };

        let exit = result
            .llm_error
            .as_ref()
            .map_or(SubagentExit::Completed, |reason| SubagentExit::Failed {
                reason: reason.clone(),
            });
        Ok(SubagentResult {
            child_agent_id: Some(child_agent_id),
            final_message: result.total_text,
            total_input_tokens: result.total_input_tokens,
            total_output_tokens: result.total_output_tokens,
            files_changed: result
                .file_changes
                .into_iter()
                .map(|change| change.path)
                .collect(),
            exit,
        })
    }
}

fn narrow_permissions(parent: &AgentPermissions, kind: &SubagentKindSpec) -> AgentPermissions {
    let capabilities = parent
        .capabilities
        .iter()
        .filter(|held| {
            kind.allowed_capabilities
                .iter()
                .any(|allowed| held.satisfies(allowed))
        })
        .cloned()
        .collect();
    AgentPermissions {
        scope: parent.scope.clone(),
        capabilities,
    }
}

fn narrowed_tool_permissions(
    request: &SubagentDispatchRequest,
    kind: &SubagentKindSpec,
) -> AgentToolPermissions {
    let mut per_tool = BTreeMap::new();
    for tool in &kind.allowed_tools {
        let parent_state = resolve_effective_permission(
            &request.user_tool_defaults,
            request.parent_tool_permissions.as_ref(),
            tool,
        );
        per_tool.insert(tool.clone(), parent_state);
    }
    AgentToolPermissions { per_tool }
}

fn policy_for(
    permissions: AgentPermissions,
    tool_permissions: AgentToolPermissions,
    request: &SubagentDispatchRequest,
) -> PolicyConfig {
    let fallback = match request.user_tool_defaults.mode {
        UserDefaultMode::AutoReview => ToolState::Ask,
        _ => ToolState::Deny,
    };
    let user_default = UserToolDefaults::default_permissions(BTreeMap::new(), fallback);
    PolicyConfig::default()
        .with_agent_permissions(permissions)
        .with_user_default(user_default)
        .with_agent_override(Some(tool_permissions))
}

fn loop_config_for(kind: &SubagentKindSpec) -> AgentLoopConfig {
    let mut config = AgentLoopConfig {
        system_prompt: kind.system_prompt.clone(),
        max_iterations: kind.budget.max_iterations as usize,
        ..AgentLoopConfig::default()
    };
    if let Some(model) = kind.default_model.clone() {
        config.model = model;
    }
    if let Some(max_tokens) = kind.budget.max_tokens {
        config.max_tokens = max_tokens;
    }
    config
}

fn system_prompt_for(kind: &SubagentKindSpec, request: &SubagentDispatchRequest) -> String {
    match request.system_prompt_addendum.as_deref() {
        Some(addendum) if !addendum.trim().is_empty() => {
            format!("{}\n\n{}", kind.system_prompt, addendum)
        }
        _ => kind.system_prompt.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core::{AgentId, AgentScope, Capability};

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
    fn policy_fallback_denies_tools_outside_allowlist() {
        let request = SubagentDispatchRequest {
            parent_agent_id: AgentId::generate(),
            subagent_type: "explore".into(),
            prompt: "inspect".into(),
            originating_user_id: Some("user".into()),
            parent_chain: Vec::new(),
            model_override: None,
            system_prompt_addendum: None,
            parent_permissions: AgentPermissions {
                scope: AgentScope::default(),
                capabilities: vec![Capability::SpawnAgent],
            },
            parent_tool_permissions: None,
            user_tool_defaults: UserToolDefaults::full_access(),
        };
        let registry = SubagentRegistry::bundled();
        let kind = registry.get("explore").unwrap();
        let tool_permissions = narrowed_tool_permissions(&request, kind);
        let policy = policy_for(AgentPermissions::empty(), tool_permissions, &request);
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
}
