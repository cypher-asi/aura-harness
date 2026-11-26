//! [`AgentRunnerConfig`]: model, budget, and routing settings for the runner.

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the `AgentRunner`.
#[derive(Debug, Clone)]
pub struct AgentRunnerConfig {
    pub max_agentic_iterations: usize,
    /// Explicit selected effort, independent of the response token allowance.
    pub reasoning_effort: Option<aura_model::ThinkingEffort>,
    pub stream_timeout_secs: u64,
    pub max_context_tokens: u64,
    pub max_task_credits: Option<u64>,
    pub default_model: String,
    pub simple_model: String,
    /// JWT auth token for proxy-mode LLM routing.
    pub auth_token: Option<String>,
    /// Org UUID forwarded as the `X-Aura-Org-Id` header on outbound
    /// `/v1/messages` calls so `aura-router` can bucket per-org rate
    /// limits / billing on automaton runs the same way it does for
    /// interactive chat (where `SessionState::aura_org_id` already
    /// flows through `AgentLoopConfig::aura_org_id`). Without this
    /// header `aura-router` falls back to per-IP buckets, which is
    /// what makes burst-y eval automation traffic trip Cloudflare on
    /// `aura-router.onrender.com` even when chat from the same
    /// account does not.
    pub aura_org_id: Option<String>,
    /// Storage session UUID forwarded as the `X-Aura-Session-Id`
    /// header on outbound `/v1/messages` calls. Mirrors the chat
    /// path's `SessionState::aura_session_id`, generated per
    /// automaton-start so concurrent runs of the same agent get
    /// distinct billing/observability partitions.
    pub aura_session_id: Option<String>,
    /// Template agent UUID forwarded as the `X-Aura-Agent-Id` header
    /// on outbound `/v1/messages` calls. Mirrors the chat path's
    /// `SessionState::aura_agent_id` (populated from
    /// `RuntimeRequest.project.aura_agent_id`). Without this, automaton runs
    /// hit `aura-router` without an agent identity and Cloudflare's
    /// WAF treats them as unsanctioned API traffic, returning the
    /// HTML challenge page (status 403) that's been stalling SWE-bench.
    pub aura_agent_id: Option<String>,
    /// Project UUID forwarded as the `X-Aura-Project-Id` header on
    /// outbound `/v1/messages` calls. Mirrors the chat / task-extract
    /// paths' `SessionState::project_id` -> `aura_project_id`
    /// projection. Same WAF-bucketing rationale as
    /// [`Self::aura_agent_id`].
    pub aura_project_id: Option<String>,
    /// Stable `prompt_cache_key` forwarded to OpenAI-family routing
    /// via [`AgentLoopConfig::prompt_cache_key`]. The dev-loop and
    /// task-run paths set this to `Some(format!("devloop:{project_id}"))`
    /// so cross-task agent invocations land on the same OpenAI cache
    /// bucket. Anthropic caching does not need a key — the provider's
    /// ephemeral `cache_control` breakpoints in
    /// `aura_model::anthropic::convert` handle prefix reuse.
    pub prompt_cache_key: Option<String>,
    /// Phase 5: per-task switch for the early test-gate oracle.
    ///
    /// `TaskRunAutomaton` (and any future task-shaped automaton) sets
    /// this `true` to install
    /// [`aura_agent_steering::EarlyTestOracle`] into the
    /// per-run `SteeringRegistry` for every task this runner
    /// executes; chat / shell / generic callers leave it `false` and
    /// the oracle never fires.
    ///
    /// The wired-through path is:
    /// `TaskRunConfig.early_test_oracle` →
    /// `AgentRunnerConfig.early_test_oracle` →
    /// `AgentLoopConfig.early_test_oracle: Option<EarlyTestOracleConfig>`
    /// → `LoopState::steering`.
    pub early_test_oracle: bool,
}

impl AgentRunnerConfig {
    /// Construct an [`AgentRunnerConfig`] for an explicit, caller-supplied
    /// model. **There is no `Default` impl on purpose**: silently
    /// substituting a constant is exactly what shipped the wrong
    /// upstream model in production (the worker path took
    /// `AgentRunnerConfig::default()` and routed every dev-loop / task-run
    /// turn at `claude-opus-4-6` even when the user had selected
    /// `claude-opus-4-7`).
    ///
    /// `simple_model` defaults to `model` so callers that don't have a
    /// distinct cheap-tier model still get a sane starting point; pass
    /// it through [`Self::with_simple_model`] when a separate model
    /// should drive the trivial-task path.
    #[must_use]
    pub fn for_agent(model: impl Into<String>) -> Self {
        let model = model.into();
        Self {
            max_agentic_iterations: aura_core_types::MAX_TURNS as usize,
            reasoning_effort: None,
            // Matches the reasoner's default reqwest request timeout
            // (300s / `AURA_MODEL_TIMEOUT_MS`) so the outer `timeout()`
            // guard in `AgentLoop::call_model` does not preempt an
            // in-flight provider request. See the comment on
            // `AgentLoopConfig::stream_timeout`.
            stream_timeout_secs: 300,
            max_context_tokens: 200_000,
            max_task_credits: None,
            simple_model: model.clone(),
            default_model: model,
            auth_token: None,
            aura_org_id: None,
            aura_session_id: None,
            aura_agent_id: None,
            aura_project_id: None,
            prompt_cache_key: None,
            // Phase 5: default off so chat / generic runners do not
            // pay for the oracle's hint generation. Task-shaped
            // automatons flip this on per-construction.
            early_test_oracle: false,
        }
    }

    /// Override the simple-task model used for trivial classifications
    /// (see [`crate::turn_config::resolve_simple_model`]). Callers that
    /// don't care fall through to [`Self::for_agent`]'s default of
    /// echoing the primary model.
    #[must_use]
    pub fn with_simple_model(mut self, simple: impl Into<String>) -> Self {
        self.simple_model = simple.into();
        self
    }
}
