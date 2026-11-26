//! Per-agent identities the scheduler builds each turn's loop config from.

use aura_agent::AgentLoopConfig;
use aura_core_types::AgentId;
use aura_model::{ModelRequestKind, PromptCacheRetention};
use dashmap::DashMap;
use std::sync::Arc;

/// Per-agent identity recorded by the chat WS / automaton bridge / `/tx`
/// and consulted by [`Scheduler::schedule_agent_with_overrides`](super::Scheduler::schedule_agent_with_overrides) when no
/// explicit `agent_loop_config` override is supplied.
///
/// This is the seam that fixes the worker-path regression where `/v1/messages`
/// went out with `claude-opus-4-6` (the pre-rename default) and a stripped
/// `X-Aura-*` envelope, causing `aura-router` to bucket the request as anonymous
/// public traffic and return `429 RATE_LIMITED`.
#[derive(Debug, Clone)]
pub struct AgentIdentity {
    /// Explicit caller effort; inherited by worker and child runs.
    pub reasoning_effort: Option<aura_model::ThinkingEffort>,
    pub upstream_provider_family: Option<String>,
    /// Caller-selected model (e.g. `claude-opus-4-7`). Required.
    pub model: String,
    /// Org UUID forwarded as `X-Aura-Org-Id` on outbound `/v1/messages` calls.
    pub aura_org_id: Option<String>,
    /// Storage session UUID forwarded as `X-Aura-Session-Id`.
    pub aura_session_id: Option<String>,
    /// Project-agent UUID forwarded as `X-Aura-Agent-Id`.
    pub aura_agent_id: Option<String>,
    /// Project UUID forwarded as `X-Aura-Project-Id`.
    pub aura_project_id: Option<String>,
    /// System prompt to feed into the agent loop.
    pub system_prompt: String,
    /// OpenAI-family stable cache key.
    pub prompt_cache_key: Option<String>,
    /// Retention hint paired with `prompt_cache_key`.
    pub prompt_cache_retention: Option<PromptCacheRetention>,
    /// Request contract kind. Chat sessions ship `Chat`; dev-loop / task-run
    /// land `DevLoopBootstrap` (and the loop self-promotes to
    /// `DevLoopContinuation` after the first iteration).
    pub request_kind: ModelRequestKind,
    /// Maximum context window in tokens, used for compaction.
    pub max_context_tokens: usize,
    /// JWT auth token forwarded onto outbound model requests.
    pub auth_token: Option<String>,
    /// Prompt-cache scope of the agent's kernel (see
    /// [`KernelConfig::cache_salt`]). Spawned agents inherit it with the
    /// rest of the identity, so a benchmark run's children share its
    /// cache scope and no other.
    pub cache_salt: Option<String>,
    /// Scratch folder of the agent's commands (see
    /// [`KernelConfig::run_scratch`]). Spawned agents inherit it with the
    /// rest of the identity, so a run's children work in its scratch.
    pub run_scratch: Option<aura_exec_tools::RunScratch>,
}

impl AgentIdentity {
    /// Build the per-agent [`AgentLoopConfig`] consumed by the worker
    /// path. Mirrors the chat-WS path's `Session::agent_loop_config`
    /// shape: every router/billing identifier round-trips, the
    /// caller-selected model is honored, and `request_kind` matches
    /// what the call site declared.
    #[must_use]
    pub fn into_loop_config(self) -> AgentLoopConfig {
        AgentLoopConfig {
            user_thinking_effort: self.reasoning_effort,
            upstream_provider_family: self.upstream_provider_family,
            system_prompt: self.system_prompt,
            max_context_tokens: Some(self.max_context_tokens as u64),
            auth_token: self.auth_token,
            aura_project_id: self.aura_project_id,
            aura_agent_id: self.aura_agent_id,
            aura_session_id: self.aura_session_id,
            aura_org_id: self.aura_org_id,
            prompt_cache_key: self.prompt_cache_key,
            prompt_cache_retention: self.prompt_cache_retention.map(|r| match r {
                PromptCacheRetention::Hours24 => "24h".to_string(),
                PromptCacheRetention::InMemory => "in_memory".to_string(),
            }),
            request_kind: self.request_kind,
            ..AgentLoopConfig::for_agent(self.model)
        }
    }
}

/// In-memory registry of [`AgentIdentity`] entries.
///
/// Populated in three places:
/// - `Session::apply_chat_runtime_request` (chat run bootstrap) — registers
///   the session model + IDs before the first turn dispatches.
/// - `automaton_bridge::start_dev_loop_with_capabilities` /
///   `run_task_with_capabilities` — registers the dev-loop / task-run identity
///   alongside the existing `AgentRunnerConfig` plumbing (commit `d12fe29`).
/// - `AgentHost::spawn_agent` — registers a spawned agent's identity after
///   allocating the child id, so its turns go out with the resolved model +
///   parent IDs.
///
/// The HTTP `/tx` and `/agents/:id/tool_permissions` paths do **not** carry
/// per-call identity. They rely on the entry registered by the upstream
/// session / automaton bootstrap; if no entry exists,
/// [`Scheduler::schedule_agent_with_overrides`](super::Scheduler::schedule_agent_with_overrides) returns
/// [`SchedulerError::AgentNotRegistered`](super::SchedulerError::AgentNotRegistered) (and emits a structured
/// `error!` line) instead of falling back silently. The failing tx
/// stays in the inbox / `failed_txs` so the operator sees the
/// regression rather than a 429.
#[derive(Default)]
pub struct AgentIdentityRegistry {
    inner: DashMap<AgentId, AgentIdentity>,
}

impl AgentIdentityRegistry {
    /// Construct an empty registry.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Register or replace the identity for `agent_id`. Replacing on
    /// chat-run bootstrap re-keys are intentional — reconnecting an existing
    /// run must be allowed to refresh the model + IDs without leaking the
    /// previous bundle.
    pub fn register(&self, agent_id: AgentId, identity: AgentIdentity) {
        self.inner.insert(agent_id, identity);
    }

    /// Look up the registered identity. Returns a clone so the caller
    /// can build the per-turn `AgentLoopConfig` without holding a
    /// `DashMap` ref across the scheduling await point.
    #[must_use]
    pub fn get(&self, agent_id: AgentId) -> Option<AgentIdentity> {
        self.inner.get(&agent_id).map(|entry| entry.value().clone())
    }

    /// The cache salt registered for `agent_id`, if any.
    #[must_use]
    pub fn cache_salt(&self, agent_id: AgentId) -> Option<String> {
        self.inner
            .get(&agent_id)
            .and_then(|entry| entry.value().cache_salt.clone())
    }

    /// The scratch folder registered for `agent_id`, if any.
    #[must_use]
    pub fn run_scratch(&self, agent_id: AgentId) -> Option<aura_exec_tools::RunScratch> {
        self.inner
            .get(&agent_id)
            .and_then(|entry| entry.value().run_scratch.clone())
    }

    /// Clear the entry — primarily used by tests asserting the
    /// hard-fail path. Production code holds entries for the lifetime
    /// of the parent session / automaton.
    pub fn unregister(&self, agent_id: AgentId) {
        self.inner.remove(&agent_id);
    }
}
