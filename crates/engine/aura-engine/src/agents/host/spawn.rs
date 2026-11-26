//! `spawn_agent` and the spawn path shared with `task`.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use aura_agent::AgentLoopConfig;
use aura_agent_kernel::{ChildAgentSpec, SpawnHook, SpawnOutcome};
use aura_agent_kernel::{ExecutorRouter, KernelSpawnHook, PolicyConfig};
use aura_core_types::{
    AgentId, AgentMode, AgentPermissions, AgentToolPermissions, Transaction, TransactionType,
};
use aura_model::ToolDefinition;
use bytes::Bytes;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::agents::factory::ChildKernelRequest;
use crate::agents::hooks::AgentHooks;
use crate::agents::inbox::MessageKind;
use crate::agents::setup;

use super::{write_observed, AgentEntry, AgentEnv, Body, HostInner, RunSetup, ThreadChoice};

/// A child's loop settings and tools, before it joins the table.
struct ChildRuntime {
    loop_config: AgentLoopConfig,
    router: ExecutorRouter,
    tools: Vec<ToolDefinition>,
}

impl HostInner {
    // ------------------------------------------------------------------
    // spawn_agent
    // ------------------------------------------------------------------

    /// Create a long-lived child for `spawn_agent`, optionally giving it a
    /// first message.
    pub(in crate::agents) async fn spawn_agent(
        self: &Arc<Self>,
        parent: AgentId,
        originating_user_id: Option<&str>,
        child: ChildAgentSpec,
        initial_prompt: Option<String>,
    ) -> Result<SpawnOutcome, String> {
        let Some(parent_entry) = self.parent_snapshot(parent) else {
            return Err(format!(
                "agent {parent} is not registered with the agent host"
            ));
        };
        self.check_spawn_allowed(&parent_entry, &child)?;

        let child_id = child.preassigned_agent_id.unwrap_or_else(AgentId::generate);
        if self.agents.lock().contains_key(&child_id) {
            return Err(format!("agent {child_id} already exists"));
        }
        let plan = self.spawn_plan(parent, &parent_entry, &child, child_id);
        let outcome = self
            .create_child(plan, &parent_entry, None, originating_user_id)
            .await?;

        if let Some(prompt) = initial_prompt.filter(|p| !p.trim().is_empty()) {
            let envelope = self.envelope(
                Some(parent),
                child_id,
                MessageKind::Request,
                prompt,
                ThreadChoice::New,
            );
            self.enqueue(child_id, envelope)?;
        }
        Ok(SpawnOutcome {
            child_agent_id: child_id,
            external_agent_id: Some(child_id.to_hex()),
            delegate_tx_hash: outcome.delegate_tx_hash,
        })
    }

    /// Whether `parent_entry` may spawn `child` now: its mode, its
    /// permissions, the depth limit and the running limit.
    fn check_spawn_allowed(
        &self,
        parent_entry: &ParentSnapshot,
        child: &ChildAgentSpec,
    ) -> Result<(), String> {
        if !parent_entry.mode.allows_spawn() {
            return Err(format!(
                "{:?} mode does not allow spawning agents",
                parent_entry.mode
            ));
        }
        if !parent_entry.permissions.contains(&child.permissions) {
            return Err("a spawned agent cannot have more permissions than its parent".into());
        }
        let depth = parent_entry.chain.len() + 2;
        if depth > self.config.max_depth as usize {
            return Err(format!(
                "spawn depth {depth} exceeds the limit of {}",
                self.config.max_depth
            ));
        }
        if self.running_count() >= self.config.max_running {
            return Err(format!(
                "{} agents are already running",
                self.config.max_running
            ));
        }
        Ok(())
    }

    /// The plan for a `spawn_agent` child: the parent's mode and model,
    /// the child's own permissions and prompt.
    fn spawn_plan(
        &self,
        parent: AgentId,
        parent_entry: &ParentSnapshot,
        child: &ChildAgentSpec,
        child_id: AgentId,
    ) -> ChildPlan {
        let tool_permissions = child
            .tool_permissions
            .clone()
            .or_else(|| parent_entry.tool_permissions.clone());
        let base_prompt = child.system_prompt_override.clone().unwrap_or_else(|| {
            self.scheduler
                .identity_registry()
                .get(parent)
                .map(|identity| identity.system_prompt)
                .unwrap_or_default()
        });
        let policy = setup::agent_policy(
            child.permissions.clone(),
            tool_permissions.clone(),
            &parent_entry.env.user_tool_defaults,
            parent_entry.mode,
        );
        ChildPlan {
            child_id,
            parent,
            name: child.name.clone(),
            role: child.role.clone(),
            permissions: child.permissions.clone(),
            tool_permissions: tool_permissions.unwrap_or_default(),
            allowed_tools: None,
            mode: parent_entry.mode,
            model: parent_entry.model.clone(),
            system_prompt: setup::spawned_agent_prompt(&base_prompt, &child.name, &child.role),
            reasoning_effort: None,
            kind: None,
            iteration_cap: None,
            timeout: None,
            policy,
        }
    }

    // ------------------------------------------------------------------
    // shared spawn path
    // ------------------------------------------------------------------

    /// Snapshot of what a child inherits from `parent`.
    pub(super) fn parent_snapshot(&self, parent: AgentId) -> Option<ParentSnapshot> {
        let agents = self.agents.lock();
        let entry = agents.get(&parent)?;
        Some(ParentSnapshot {
            chain: entry.chain.clone(),
            env: entry.env.clone(),
            mode: entry.mode,
            model: entry.model.clone(),
            permissions: entry.permissions.clone(),
            tool_permissions: entry.tool_permissions.clone(),
            stop: entry.stop.clone(),
        })
    }

    /// Write the child's records, build its run setup, and add it to the
    /// table as idle.
    pub(super) async fn create_child(
        self: &Arc<Self>,
        plan: ChildPlan,
        parent: &ParentSnapshot,
        spawn_record: Option<serde_json::Value>,
        originating_user_id: Option<&str>,
    ) -> Result<SpawnOutcome, String> {
        let originating_user_id = originating_user_id
            .map(ToString::to_string)
            .or_else(|| parent.env.originating_user_id.clone());
        let outcome = self
            .write_spawn_records(&plan, parent, spawn_record, originating_user_id.as_deref())
            .await?;

        let mut chain = Vec::with_capacity(parent.chain.len() + 1);
        chain.push(plan.parent);
        chain.extend(parent.chain.iter().copied());

        let loop_config = self.child_loop_config(&plan);
        let (router, tools) = self.child_tools(&plan, parent, &chain, originating_user_id);
        let runtime = ChildRuntime {
            loop_config,
            router,
            tools,
        };
        self.insert_child(plan, parent, chain, runtime);
        Ok(outcome)
    }

    /// Under the parent's spawn lock, append the `SubagentSpawn` audit
    /// row (when there is one) and create the child's record.
    async fn write_spawn_records(
        &self,
        plan: &ChildPlan,
        parent: &ParentSnapshot,
        spawn_record: Option<serde_json::Value>,
        originating_user_id: Option<&str>,
    ) -> Result<SpawnOutcome, String> {
        let lock = self.parent_lock(plan.parent);
        let _guard = lock.lock().await;
        if let Some(record) = spawn_record {
            let tx = Transaction::new_chained(
                plan.parent,
                TransactionType::SubagentSpawn,
                Bytes::from(record.to_string().into_bytes()),
                None,
            );
            write_observed(
                &self.store,
                self.scheduler.record_observer(),
                plan.parent,
                &tx,
            )
            .map_err(|e| format!("spawn record: {e}"))?;
        }
        KernelSpawnHook::new(self.store.clone())
            .spawn_child(
                &plan.parent,
                originating_user_id,
                ChildAgentSpec {
                    name: plan.name.clone(),
                    role: plan.role.clone(),
                    permissions: plan.permissions.clone(),
                    tool_permissions: Some(plan.tool_permissions.clone()),
                    parent_tool_permissions: parent.tool_permissions.clone(),
                    system_prompt_override: Some(plan.system_prompt.clone()),
                    preassigned_agent_id: Some(plan.child_id),
                    initial_prompt: None,
                },
            )
            .await
            .map_err(|e| format!("spawn child: {e}"))
    }

    /// Register the child's identity (from its parent's, with the plan's
    /// model and prompt) and build its loop settings.
    fn child_loop_config(&self, plan: &ChildPlan) -> AgentLoopConfig {
        let identity = self
            .scheduler
            .identity_registry()
            .get(plan.parent)
            .map(|mut identity| {
                identity.model.clone_from(&plan.model);
                if let Some(effort) = plan.reasoning_effort {
                    identity.reasoning_effort = Some(effort);
                }
                identity.system_prompt.clone_from(&plan.system_prompt);
                self.scheduler
                    .identity_registry()
                    .register(plan.child_id, identity.clone());
                identity
            });
        let mut loop_config = setup::loop_config_for(&plan.model, &plan.system_prompt, identity);
        if let Some(kind) = plan.kind.as_ref() {
            setup::apply_kind_budget(&mut loop_config, kind);
        }
        if let Some(effort) = plan.reasoning_effort {
            loop_config.user_thinking_effort = Some(effort);
        }
        if let Some(cap) = plan.iteration_cap {
            loop_config.max_iterations = loop_config.max_iterations.min(cap as usize);
        }
        loop_config
    }

    /// Build the child's kernel tools and attach the host's hooks to them.
    fn child_tools(
        self: &Arc<Self>,
        plan: &ChildPlan,
        parent: &ParentSnapshot,
        chain: &[AgentId],
        originating_user_id: Option<String>,
    ) -> (ExecutorRouter, Vec<ToolDefinition>) {
        let request = ChildKernelRequest {
            child_agent_id: plan.child_id,
            permissions: plan.permissions.clone(),
            tool_permissions: plan.tool_permissions.clone(),
            user_tool_defaults: parent.env.user_tool_defaults.clone(),
            parent_chain: chain.to_vec(),
            originating_user_id: originating_user_id.clone(),
            model_id: plan.model.clone(),
        };
        let kernel = parent.env.factory.build_child(&request);
        let hooks = AgentHooks::new(Arc::downgrade(self), plan.child_id);
        let mut resolver = hooks
            .attach_with_env(kernel.resolver, &parent.env)
            .with_caller_permissions(plan.permissions.clone())
            .with_tool_permission_context(
                parent.env.user_tool_defaults.clone(),
                Some(plan.tool_permissions.clone()),
            )
            .with_parent_chain(chain.to_vec());
        if let Some(user) = originating_user_id.filter(|u| !u.trim().is_empty()) {
            resolver = resolver.with_originating_user_id(user);
        }
        if !plan.model.trim().is_empty() {
            resolver = resolver.with_caller_model_id(plan.model.clone());
        }
        let mut tools = kernel.tools;
        if let Some(allowed) = &plan.allowed_tools {
            tools.retain(|tool| allowed.contains(&tool.name));
        }
        (crate::executor::build_executor_router(resolver), tools)
    }

    /// Add the new child to the table as idle.
    fn insert_child(
        &self,
        plan: ChildPlan,
        parent: &ParentSnapshot,
        chain: Vec<AgentId>,
        runtime: ChildRuntime,
    ) {
        let one_shot = plan.kind.is_some();
        let setup = RunSetup {
            loop_config: runtime.loop_config,
            policy: plan.policy,
            router: runtime.router,
            tools: runtime.tools,
            timeout: plan.timeout,
        };
        info!(
            child = %plan.child_id,
            parent = %plan.parent,
            name = %plan.name,
            depth = chain.len(),
            "agent spawned"
        );
        self.agents.lock().insert(
            plan.child_id,
            AgentEntry {
                name: plan.name,
                role: plan.role,
                chain,
                permissions: plan.permissions,
                tool_permissions: Some(plan.tool_permissions),
                mode: plan.mode,
                model: plan.model,
                env: parent.env.clone(),
                stop: parent.stop.child_token(),
                one_shot,
                running: false,
                paused: false,
                turns: 0,
                last_reply: None,
                parent_thread: None,
                body: Body::Hosted {
                    setup: Arc::new(setup),
                    history: Vec::new(),
                    queue: VecDeque::new(),
                },
            },
        );
    }
}

/// What a new child is, before it is set up.
pub(super) struct ChildPlan {
    pub(super) child_id: AgentId,
    pub(super) parent: AgentId,
    pub(super) name: String,
    pub(super) role: String,
    pub(super) permissions: AgentPermissions,
    pub(super) tool_permissions: AgentToolPermissions,
    pub(super) allowed_tools: Option<Vec<String>>,
    pub(super) mode: AgentMode,
    pub(super) model: String,
    pub(super) system_prompt: String,
    pub(super) reasoning_effort: Option<aura_model::ThinkingEffort>,
    pub(super) kind: Option<aura_core_types::SubagentKindSpec>,
    pub(super) iteration_cap: Option<u32>,
    pub(super) timeout: Option<Duration>,
    pub(super) policy: PolicyConfig,
}

pub(super) struct ParentSnapshot {
    pub(super) chain: Vec<AgentId>,
    pub(super) env: AgentEnv,
    pub(super) mode: AgentMode,
    pub(super) model: String,
    pub(super) permissions: AgentPermissions,
    pub(super) tool_permissions: Option<AgentToolPermissions>,
    pub(super) stop: CancellationToken,
}
