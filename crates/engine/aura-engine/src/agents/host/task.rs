//! The `task` tool: spawn a child of a bundled kind and wait for its result.

use std::sync::Arc;
use std::time::Duration;

use aura_agent::AgentLoopEvent;
use aura_agent_kernel::PolicyConfig;
use aura_agent_subagent::{
    narrow_permissions, overrides_from_request, parent_context_from_request, OverrideManifest,
    SubagentDerivation, SubagentSpec,
};
use aura_core_types::{
    AgentId, AgentPermissions, AgentToolPermissions, SpawnMode, SubagentDispatchRequest,
    SubagentKindSpec, SubagentResult,
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::agents::inbox::MessageKind;
use crate::agents::setup;

use super::outcome::cancelled;
use super::spawn::{ChildPlan, ParentSnapshot};
use super::{AgentEnv, HostInner, ThreadChoice};

/// A validated `task` request, ready to create its child.
struct TaskPlan {
    plan: ChildPlan,
    parent_entry: ParentSnapshot,
    /// The `SubagentSpawn` audit record.
    record: serde_json::Value,
}

/// What a `task` child may do, narrowed from its spec and kind.
struct TaskAccess {
    permissions: AgentPermissions,
    tool_permissions: AgentToolPermissions,
    allowed_tools: Vec<String>,
    policy: PolicyConfig,
}

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
        let TaskPlan {
            plan,
            parent_entry,
            record,
        } = match self.plan_task(&request) {
            Ok(task) => task,
            Err(reason) => return SubagentResult::rejected(reason),
        };
        let child_id = plan.child_id;
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
        self.start_task(request, child_id, events, cancel).await
    }

    /// Check a `task` request against the host and its parent, and plan
    /// the child it asks for. `Err` is the reason to reject it.
    fn plan_task(&self, request: &SubagentDispatchRequest) -> Result<TaskPlan, String> {
        let parent = request.parent_agent_id;
        let Some(kind) = self.kinds.get(&request.subagent_type).cloned() else {
            return Err(format!("unknown subagent type '{}'", request.subagent_type));
        };
        let Some(parent_entry) = self.parent_snapshot(parent) else {
            return Err(format!(
                "agent {parent} is not registered with the agent host"
            ));
        };
        let (spec, manifest) = self.derive_task_spec(request, &kind)?;
        let reasoning_effort = parse_reasoning_effort(spec.reasoning_effort.as_deref())?;
        if self.running_count() >= self.config.max_running {
            return Err(format!(
                "spawn rejected: {} agents are already running",
                self.config.max_running
            ));
        }
        let child_id = AgentId::generate();
        let plan = task_child_plan(
            request,
            kind,
            &spec,
            &parent_entry.env,
            child_id,
            reasoning_effort,
        );
        let record = serde_json::json!({
            "parent_agent_id": parent,
            "child_agent_id": child_id,
            "override_manifest": manifest,
            "spawn_mode": request.spawn_mode.unwrap_or_default(),
        });
        Ok(TaskPlan {
            plan,
            parent_entry,
            record,
        })
    }

    /// Derive the child's spec from the parent's context and the request's
    /// overrides, rejecting what the parent may not spawn.
    fn derive_task_spec(
        &self,
        request: &SubagentDispatchRequest,
        kind: &SubagentKindSpec,
    ) -> Result<(SubagentSpec, OverrideManifest), String> {
        let parent_ctx = parent_context_from_request(request);
        if !parent_ctx.mode.allows_spawn() {
            return Err(format!(
                "spawn rejected: {:?} mode does not allow spawning agents",
                parent_ctx.mode
            ));
        }
        let (spec, manifest) = self
            .derivation
            .derive(&parent_ctx, overrides_from_request(request, kind))
            .map_err(|err| format!("spawn rejected: {err}"))?;
        if spec.budget.max_iterations == 0 {
            return Err("child execution limits must be positive".into());
        }
        Ok((spec, manifest))
    }

    /// Give a created `task` child its prompt: return at once when the
    /// request is detached, otherwise wait for the child's result.
    async fn start_task(
        self: &Arc<Self>,
        request: SubagentDispatchRequest,
        child_id: AgentId,
        events: Option<mpsc::Sender<AgentLoopEvent>>,
        cancel: Option<CancellationToken>,
    ) -> SubagentResult {
        let parent = request.parent_agent_id;
        let detached = matches!(request.spawn_mode, Some(SpawnMode::Detached));
        let mut envelope = self.envelope(
            Some(parent),
            child_id,
            MessageKind::Request,
            request.prompt,
            ThreadChoice::New,
        );
        envelope.events = events;
        if detached {
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
        envelope.waiter = Some(tx);
        envelope.cancel = cancel;
        if let Err(err) = self.enqueue(child_id, envelope) {
            return SubagentResult::rejected(err);
        }
        rx.await.unwrap_or_else(|_| cancelled(child_id))
    }
}

/// The spec's reasoning effort, if it names one. `Err` is the reason
/// to reject an effort the model layer does not know.
fn parse_reasoning_effort(
    effort: Option<&str>,
) -> Result<Option<aura_model::ThinkingEffort>, String> {
    match effort {
        None => Ok(None),
        Some(effort) => match aura_model::ThinkingEffort::from_wire(effort) {
            Some(effort) => Ok(Some(effort)),
            None => Err(format!("invalid reasoning effort: {effort}")),
        },
    }
}

/// The plan for a `task` child of `kind`, from its derived `spec`.
fn task_child_plan(
    request: &SubagentDispatchRequest,
    kind: SubagentKindSpec,
    spec: &SubagentSpec,
    env: &AgentEnv,
    child_id: AgentId,
    reasoning_effort: Option<aura_model::ThinkingEffort>,
) -> ChildPlan {
    let access = task_access(spec, &kind, env);
    let model = task_model(spec, &kind);
    let timeout_ms = request
        .override_budget
        .as_ref()
        .map_or(kind.budget.timeout_ms, |b| b.timeout_ms);
    ChildPlan {
        child_id,
        parent: request.parent_agent_id,
        name: kind.name.clone(),
        role: format!("subagent:{}", kind.name),
        permissions: access.permissions,
        tool_permissions: access.tool_permissions,
        allowed_tools: Some(access.allowed_tools),
        mode: spec.mode,
        model,
        system_prompt: setup::kind_system_prompt(&kind, spec.system_prompt_addendum.as_deref()),
        reasoning_effort,
        iteration_cap: Some(spec.budget.max_iterations),
        kind: Some(kind),
        timeout: Some(Duration::from_millis(timeout_ms)),
        policy: access.policy,
    }
}

/// Narrow the child's permissions, tools and policy from `spec`, its
/// `kind` and the tree's defaults in `env`.
fn task_access(spec: &SubagentSpec, kind: &SubagentKindSpec, env: &AgentEnv) -> TaskAccess {
    let permissions = narrow_permissions(
        &AgentPermissions {
            scope: spec.permissions.scope.clone(),
            capabilities: spec.permissions.capabilities.clone(),
        },
        kind,
    );
    let user_tool_defaults = spec
        .user_tool_defaults
        .clone()
        .unwrap_or_else(|| env.user_tool_defaults.clone());
    let allowed_tools = spec
        .tool_subset
        .clone()
        .unwrap_or_else(|| kind.allowed_tools.clone());
    let tool_permissions = setup::narrowed_tool_permissions(
        spec.parent_tool_permissions.as_ref(),
        &user_tool_defaults,
        &allowed_tools,
    );
    let policy = setup::task_policy(
        permissions.clone(),
        tool_permissions.clone(),
        &user_tool_defaults,
        spec.mode,
    );
    TaskAccess {
        permissions,
        tool_permissions,
        allowed_tools,
        policy,
    }
}

/// The spec's model, else the kind's default, else the env fallback.
fn task_model(spec: &SubagentSpec, kind: &SubagentKindSpec) -> String {
    if spec.model_id.is_empty() {
        kind.default_model
            .clone()
            .unwrap_or_else(|| aura_model::ENV_FALLBACK_MODEL.to_string())
    } else {
        spec.model_id.clone()
    }
}
