//! The `task` tool: spawn a child of a bundled kind and wait for its result.

use std::sync::Arc;
use std::time::Duration;

use aura_agent::AgentLoopEvent;
use aura_agent_subagent::{
    narrow_permissions, overrides_from_request, parent_context_from_request, SubagentDerivation,
};
use aura_core_types::{
    AgentId, AgentPermissions, SpawnMode, SubagentDispatchRequest, SubagentResult,
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::agents::inbox::MessageKind;
use crate::agents::setup;

use super::run::cancelled;
use super::spawn::ChildPlan;
use super::{HostInner, ThreadChoice};

impl HostInner {
    // ------------------------------------------------------------------
    // task
    // ------------------------------------------------------------------

    /// Run a `task`: spawn a child of the requested kind and give it the
    /// prompt. Waits for the result unless `spawn_mode` is detached.
    pub(in crate::agents) async fn dispatch_task(
        self: &Arc<Self>,
        request: SubagentDispatchRequest,
        events: Option<mpsc::Sender<AgentLoopEvent>>,
        cancel: Option<CancellationToken>,
    ) -> SubagentResult {
        let parent = request.parent_agent_id;
        let Some(kind) = self.kinds.get(&request.subagent_type).cloned() else {
            return SubagentResult::rejected(format!(
                "unknown subagent type '{}'",
                request.subagent_type
            ));
        };
        let Some(parent_entry) = self.parent_snapshot(parent) else {
            return SubagentResult::rejected(format!(
                "agent {parent} is not registered with the agent host"
            ));
        };
        let parent_ctx = parent_context_from_request(&request);
        if !parent_ctx.mode.allows_spawn() {
            return SubagentResult::rejected(format!(
                "spawn rejected: {:?} mode does not allow spawning agents",
                parent_ctx.mode
            ));
        }
        let (spec, manifest) = match self
            .derivation
            .derive(&parent_ctx, overrides_from_request(&request, &kind))
        {
            Ok(derived) => derived,
            Err(err) => return SubagentResult::rejected(format!("spawn rejected: {err}")),
        };
        if spec.budget.max_iterations == 0 {
            return SubagentResult::rejected("child execution limits must be positive");
        }
        let reasoning_effort = match spec.reasoning_effort.as_deref() {
            None => None,
            Some(effort) => match aura_model::ThinkingEffort::from_wire(effort) {
                Some(effort) => Some(effort),
                None => {
                    return SubagentResult::rejected(format!("invalid reasoning effort: {effort}"))
                }
            },
        };
        if self.running_count() >= self.config.max_running {
            return SubagentResult::rejected(format!(
                "spawn rejected: {} agents are already running",
                self.config.max_running
            ));
        }

        let permissions = narrow_permissions(
            &AgentPermissions {
                scope: spec.permissions.scope.clone(),
                capabilities: spec.permissions.capabilities.clone(),
            },
            &kind,
        );
        let user_tool_defaults = spec
            .user_tool_defaults
            .clone()
            .unwrap_or_else(|| parent_entry.env.user_tool_defaults.clone());
        let allowed_tools = spec
            .tool_subset
            .clone()
            .unwrap_or_else(|| kind.allowed_tools.clone());
        let tool_permissions = setup::narrowed_tool_permissions(
            spec.parent_tool_permissions.as_ref(),
            &user_tool_defaults,
            &allowed_tools,
        );
        let model = if spec.model_id.is_empty() {
            kind.default_model
                .clone()
                .unwrap_or_else(|| aura_model::ENV_FALLBACK_MODEL.to_string())
        } else {
            spec.model_id.clone()
        };
        let timeout_ms = request
            .override_budget
            .as_ref()
            .map_or(kind.budget.timeout_ms, |b| b.timeout_ms);
        let policy = setup::task_policy(
            permissions.clone(),
            tool_permissions.clone(),
            &user_tool_defaults,
            spec.mode,
        );
        let child_id = AgentId::generate();
        let detached = matches!(request.spawn_mode, Some(SpawnMode::Detached));

        let plan = ChildPlan {
            child_id,
            parent,
            name: kind.name.clone(),
            role: format!("subagent:{}", kind.name),
            permissions,
            tool_permissions,
            allowed_tools: Some(allowed_tools),
            mode: spec.mode,
            model,
            system_prompt: setup::kind_system_prompt(&kind, spec.system_prompt_addendum.as_deref()),
            reasoning_effort,
            iteration_cap: Some(spec.budget.max_iterations),
            kind: Some(kind),
            timeout: Some(Duration::from_millis(timeout_ms)),
            policy,
        };

        let record = serde_json::json!({
            "parent_agent_id": parent,
            "child_agent_id": child_id,
            "override_manifest": manifest,
            "spawn_mode": request.spawn_mode.unwrap_or_default(),
        });
        if let Err(err) = self
            .create_child(
                plan,
                &parent_entry,
                Some(record),
                request.originating_user_id.as_deref(),
            )
            .await
        {
            return SubagentResult::rejected(format!("spawn failed: {err}"));
        }

        if detached {
            let mut envelope = self.envelope(
                Some(parent),
                child_id,
                MessageKind::Request,
                request.prompt,
                ThreadChoice::New,
            );
            envelope.events = events;
            if let Err(err) = self.enqueue(child_id, envelope) {
                return SubagentResult::rejected(err);
            }
            return SubagentResult::completed(
                child_id,
                format!(
                    "Started agent {child_id} in the background. Its result will be delivered \
                     to you as a message when it finishes."
                ),
            );
        }

        let (tx, rx) = oneshot::channel();
        let mut envelope = self.envelope(
            Some(parent),
            child_id,
            MessageKind::Request,
            request.prompt,
            ThreadChoice::New,
        );
        envelope.waiter = Some(tx);
        envelope.events = events;
        envelope.cancel = cancel;
        if let Err(err) = self.enqueue(child_id, envelope) {
            return SubagentResult::rejected(err);
        }
        rx.await.unwrap_or_else(|_| cancelled(child_id))
    }
}
