//! Dev-loop automaton — runs project tasks in order.
//!
//! Intentionally minimal: fetch all tasks on first tick, drop the ones
//! already marked `done`, sort the rest by `order`, then execute one
//! per tick through [`aura_agent::agent_runner::AgentRunner`]. Status
//! transitions are best-effort writes to the domain API. No retries,
//! no dependency graph, no DoD aggregates, no commit gates, no
//! preflight — those layers belong in higher-level orchestration.
//!
//! `mod.rs` owns the [`DevLoopAutomaton`] façade and [`DevLoopConfig`].
//! The Automaton trait impl and per-task execution live in [`tick`].
//! [`forward_event`] translates `aura_agent::AgentLoopEvent` into
//! `AutomatonEvent` for the WS stream and is also re-used by
//! `task_run.rs` and `chat.rs`.

use std::sync::Arc;

use aura_agent::agent_runner::{AgentRunner, AgentRunnerConfig};
use aura_reasoner::ModelProvider;
use aura_tools::catalog::ToolCatalog;
use aura_tools::domain_tools::DomainApi;

use crate::error::AutomatonError;

mod forward_event;
mod tick;

#[cfg(test)]
mod tests;

pub use forward_event::forward_agent_event;

// Per-automaton state keys. Only two cross-tick values survive the
// simplification: the queue of remaining task IDs and an initialized
// flag so the first tick can populate it. Counters are kept so the
// `LoopFinished` terminal event still reports completed/failed totals
// the UI surfaces.
const STATE_INITIALIZED: &str = "initialized";
const STATE_TASK_QUEUE: &str = "task_queue";
const STATE_COMPLETED_COUNT: &str = "completed_count";
const STATE_FAILED_COUNT: &str = "failed_count";
const STATE_LOOP_FINISHED: &str = "loop_finished";

pub(crate) struct DevLoopConfig {
    pub(crate) project_id: String,
    #[allow(dead_code)]
    agent_instance_id: String,
    #[allow(dead_code)]
    model: String,
}

impl DevLoopConfig {
    fn from_json(config: &serde_json::Value) -> Result<Self, AutomatonError> {
        let project_id = config
            .get("project_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AutomatonError::InvalidConfig("missing project_id".into()))?
            .to_string();
        let agent_instance_id = config
            .get("agent_instance_id")
            .and_then(|v| v.as_str())
            .unwrap_or("default")
            .to_string();
        let model = config
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or(aura_agent::DEFAULT_MODEL)
            .to_string();
        Ok(Self {
            project_id,
            agent_instance_id,
            model,
        })
    }
}

pub struct DevLoopAutomaton {
    pub(crate) domain: Arc<dyn DomainApi>,
    pub(crate) provider: Arc<dyn ModelProvider>,
    pub(crate) runner: AgentRunner,
    pub(crate) catalog: Arc<ToolCatalog>,
    pub(crate) tool_executor: Option<Arc<dyn aura_agent::types::AgentToolExecutor>>,
}

impl DevLoopAutomaton {
    /// Construct a dev-loop automaton bound to a kernel-mediated model
    /// provider.
    ///
    /// The `RecordingModelProvider` bound (sealed in `aura-agent`,
    /// Invariant §1 / §3) means external crates can satisfy this only
    /// by passing an `Arc<aura_agent::KernelModelGateway>`, so a raw
    /// HTTP provider can never reach the dev loop without going through
    /// `Kernel::reason_streaming` first.
    pub fn new<P>(
        domain: Arc<dyn DomainApi>,
        provider: Arc<P>,
        config: AgentRunnerConfig,
        catalog: Arc<ToolCatalog>,
    ) -> Self
    where
        P: aura_agent::RecordingModelProvider + Send + Sync + 'static,
    {
        let provider: Arc<dyn ModelProvider> = provider;
        Self {
            domain,
            provider,
            runner: AgentRunner::new(config),
            catalog,
            tool_executor: None,
        }
    }

    #[must_use]
    pub fn with_tool_executor(
        mut self,
        executor: Arc<dyn aura_agent::types::AgentToolExecutor>,
    ) -> Self {
        self.tool_executor = Some(executor);
        self
    }
}
