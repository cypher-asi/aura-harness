//! [`AgentHooks`]: the tool-facing side of the host. One value acts for
//! one calling agent and implements every agent tool hook.

use std::sync::{Arc, Weak};

use async_trait::async_trait;
use aura_agent::AgentLoopEvent;
use aura_agent_kernel::{ChildAgentSpec, SpawnError, SpawnHook, SpawnOutcome};
use aura_core_types::{AgentId, SubagentDispatchRequest, SubagentResult};
use aura_tools::{AgentControlHook, AgentReadHook, SubagentDispatchHook, ToolResolver};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::host::{AgentEnv, HostInner, LifecycleAction};
use super::inbox::MessageKind;

/// Agent tool hooks acting for one caller.
#[derive(Clone)]
pub struct AgentHooks {
    host: Weak<HostInner>,
    caller: AgentId,
}

impl AgentHooks {
    pub(super) fn new(host: Weak<HostInner>, caller: AgentId) -> Self {
        Self { host, caller }
    }

    /// Agent these hooks act for.
    #[must_use]
    pub fn caller(&self) -> AgentId {
        self.caller
    }

    fn host(&self) -> Result<Arc<HostInner>, String> {
        self.host
            .upgrade()
            .ok_or_else(|| "the agent host has shut down".to_string())
    }

    fn env(&self) -> Option<AgentEnv> {
        self.host
            .upgrade()
            .and_then(|host| host.env_of(self.caller))
    }

    /// Run a `task` for the caller, streaming the child's first turn to
    /// `events` and cancelling it with `cancel` (wait mode only).
    ///
    /// # Errors
    /// Only when the host is gone; rejections come back as a
    /// [`SubagentResult`].
    pub async fn dispatch_with_events(
        &self,
        request: SubagentDispatchRequest,
        events: Option<mpsc::Sender<AgentLoopEvent>>,
        cancel: Option<CancellationToken>,
    ) -> Result<SubagentResult, String> {
        if request.parent_agent_id != self.caller {
            return Ok(SubagentResult::rejected(
                "task: parent agent does not match the caller",
            ));
        }
        let host = self.host()?;
        Ok(host.dispatch_task(request, events, cancel).await)
    }

    /// Attach the spawn, task, messaging, and read hooks to `resolver`.
    /// The caller must already be registered with the host.
    #[must_use]
    pub fn attach(&self, resolver: ToolResolver) -> ToolResolver {
        match self.env() {
            Some(env) => self.attach_with_env(resolver, &env),
            None => self.attach_local(resolver),
        }
    }

    pub(super) fn attach_with_env(&self, resolver: ToolResolver, env: &AgentEnv) -> ToolResolver {
        let resolver = self.attach_local(resolver);
        match &env.spawn_override {
            Some(spawn) => resolver.with_spawn_hook(spawn.clone()),
            None => resolver,
        }
    }

    fn attach_local(&self, resolver: ToolResolver) -> ToolResolver {
        let me = Arc::new(self.clone());
        resolver
            .with_spawn_hook(me.clone())
            .with_subagent_dispatch_hook(me.clone())
            .with_agent_control_hook(me.clone())
            .with_agent_read_hook(me)
    }

    /// Resolve `target` to a local agent other than the caller.
    fn local_target(&self, host: &HostInner, target: &str) -> Result<Option<AgentId>, String> {
        match host.resolve(self.caller, target) {
            Some(id) if id == self.caller => Err("an agent cannot message itself".to_string()),
            other => Ok(other),
        }
    }

    fn unknown(target: &str) -> String {
        format!("unknown agent '{target}'; call list_agents to see the agents you can reach")
    }
}

#[async_trait]
impl SubagentDispatchHook for AgentHooks {
    async fn dispatch(&self, request: SubagentDispatchRequest) -> Result<SubagentResult, String> {
        self.dispatch_with_events(request, None, None).await
    }
}

#[async_trait]
impl SpawnHook for AgentHooks {
    async fn spawn_child(
        &self,
        parent_agent_id: &AgentId,
        originating_user_id: Option<&str>,
        child: ChildAgentSpec,
    ) -> Result<SpawnOutcome, SpawnError> {
        if *parent_agent_id != self.caller {
            return Err(SpawnError::Other(
                "spawn_agent: parent agent does not match the caller".into(),
            ));
        }
        let host = self.host().map_err(SpawnError::Other)?;
        let initial_prompt = child.initial_prompt.clone();
        host.spawn_agent(self.caller, originating_user_id, child, initial_prompt)
            .await
            .map_err(SpawnError::Other)
    }
}

#[async_trait]
impl AgentControlHook for AgentHooks {
    async fn deliver_message(
        &self,
        target_agent_id: &str,
        parent_agent_id: Option<&str>,
        originating_user_id: Option<&str>,
        project_id: Option<&str>,
        content: &str,
        attachments: Option<serde_json::Value>,
        model: Option<&str>,
    ) -> Result<(), String> {
        let host = self.host()?;
        if let Some(target) = self.local_target(&host, target_agent_id)? {
            return host.deliver_message(
                Some(self.caller),
                target,
                MessageKind::Request,
                content.to_string(),
            );
        }
        match self.env().and_then(|env| env.remote_control) {
            Some(remote) => {
                remote
                    .deliver_message(
                        target_agent_id,
                        parent_agent_id,
                        originating_user_id,
                        project_id,
                        content,
                        attachments,
                        model,
                    )
                    .await
            }
            None => Err(Self::unknown(target_agent_id)),
        }
    }

    async fn lifecycle(
        &self,
        target_agent_id: &str,
        parent_agent_id: Option<&str>,
        originating_user_id: Option<&str>,
        action: &str,
    ) -> Result<(), String> {
        let host = self.host()?;
        if let Some(target) = self.local_target(&host, target_agent_id)? {
            if !host.is_descendant(target, self.caller) {
                return Err(
                    "agent_lifecycle only applies to agents you started, directly or indirectly"
                        .to_string(),
                );
            }
            let action = LifecycleAction::parse(action)
                .ok_or_else(|| format!("unsupported lifecycle action '{action}'"))?;
            return host.lifecycle(target, action);
        }
        match self.env().and_then(|env| env.remote_control) {
            Some(remote) => {
                remote
                    .lifecycle(
                        target_agent_id,
                        parent_agent_id,
                        originating_user_id,
                        action,
                    )
                    .await
            }
            None => Err(Self::unknown(target_agent_id)),
        }
    }

    async fn delegate_task(
        &self,
        target_agent_id: &str,
        parent_agent_id: Option<&str>,
        originating_user_id: Option<&str>,
        task: &str,
        context: Option<&serde_json::Value>,
        model: Option<&str>,
    ) -> Result<(), String> {
        let host = self.host()?;
        if let Some(target) = self.local_target(&host, target_agent_id)? {
            let text = match context {
                Some(context) => format!(
                    "{task}\n\nContext:\n{}",
                    serde_json::to_string_pretty(context).unwrap_or_default()
                ),
                None => task.to_string(),
            };
            return host.deliver_message(Some(self.caller), target, MessageKind::Request, text);
        }
        match self.env().and_then(|env| env.remote_control) {
            Some(remote) => {
                remote
                    .delegate_task(
                        target_agent_id,
                        parent_agent_id,
                        originating_user_id,
                        task,
                        context,
                        model,
                    )
                    .await
            }
            None => Err(Self::unknown(target_agent_id)),
        }
    }
}

#[async_trait]
impl AgentReadHook for AgentHooks {
    async fn snapshot(&self, target_agent_id: &str) -> Result<serde_json::Value, String> {
        let host = self.host()?;
        if let Some(target) = host.resolve(self.caller, target_agent_id) {
            if let Some(info) = host.info(target) {
                return Ok(info.to_json());
            }
        }
        match self.env().and_then(|env| env.remote_read) {
            Some(remote) => remote.snapshot(target_agent_id).await,
            None => Err(Self::unknown(target_agent_id)),
        }
    }

    async fn list_agents(&self, org_id: Option<&str>) -> Result<serde_json::Value, String> {
        let host = self.host()?;
        let root = host.root_of(self.caller).unwrap_or(self.caller);
        let mut agents: Vec<serde_json::Value> = host
            .tree(root)
            .iter()
            .map(|info| {
                let mut value = info.to_json();
                value["is_you"] = serde_json::Value::Bool(info.agent_id == self.caller);
                value
            })
            .collect();
        if let Some(remote) = self.env().and_then(|env| env.remote_read) {
            if let Ok(serde_json::Value::Array(more)) = remote.list_agents(org_id).await {
                agents.extend(more);
            }
        }
        Ok(serde_json::Value::Array(agents))
    }
}
