//! Cost gate wiring shared by the terminal, `aura -p` and recorded runs.
//!
//! Selecting a `gate/<frontier>` model runs the frontier model as a
//! coordinator with the `run_blocks` tool; each block runs as a child
//! agent loop on a cheaper model, built here.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use aura_agent::{AgentLoop, AgentLoopConfig, KernelModelGateway, KernelToolGateway};
use aura_agent_gate::{prompts, BlockRunner, GatePolicy, GateSettings};
use aura_agent_kernel::Kernel;
use aura_config::gate::GateConfig;
use aura_core_types::{AgentId, SubagentDispatchRequest, SubagentExit, SubagentResult, Transaction};
use aura_model_reasoner::{model_catalog, Message, ThinkingEffort, ToolDefinition, Usage};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::model_selection::ModelLimits;

pub use aura_agent_gate::{frontier_of, GATE_BLOCK_AGENT, RUN_BLOCKS_TOOL};

/// Runner settings from the env, with the ledger and default policy
/// under `home`. An unreadable policy file falls back to the built-in
/// rules with a warning.
#[must_use]
pub fn settings(home: Option<&Path>, run_id: Option<String>) -> GateSettings {
    let cfg = GateConfig::from_env(home);
    let mut policy = match &cfg.policy {
        Some(path) => GatePolicy::load(path).unwrap_or_else(|e| {
            warn!(path = %path.display(), error = %e, "Gate policy unreadable; using the built-in rules");
            GatePolicy::default()
        }),
        None => GatePolicy::default(),
    };
    if let Some(rate) = cfg.explore_rate {
        policy.explore_rate = rate;
    }
    let defaults = GateSettings::default();
    GateSettings {
        policy,
        tier_overrides: cfg.tiers,
        ledger: cfg.ledger,
        run_id,
        explore_budget_usd: cfg.explore_budget_usd.unwrap_or(defaults.explore_budget_usd),
        ..defaults
    }
}

/// Runner for one surface process.
#[must_use]
pub fn runner(home: Option<&Path>, run_id: Option<String>) -> Arc<BlockRunner> {
    Arc::new(BlockRunner::new(settings(home, run_id)))
}

/// When `config.model` is a gate id, turn the turn into a coordinator
/// turn for `agent`: run the frontier model, describe the tiers in the
/// system prompt and swap the write tools for `run_blocks`. Other
/// models pass through unchanged.
pub fn coordinate(
    runner: &BlockRunner,
    agent: AgentId,
    config: &mut AgentLoopConfig,
    tools: Vec<ToolDefinition>,
    limits: Option<ModelLimits>,
) -> Vec<ToolDefinition> {
    let Some(frontier) = frontier_of(&config.model).map(str::to_string) else {
        return tools;
    };
    match limits {
        Some(limits) => limits.configure(config, &frontier),
        None => config.model.clone_from(&frontier),
    }
    runner.set_frontier(agent, &frontier);
    let tiers = runner.tiers_for(&frontier).unwrap_or_else(|e| {
        warn!(error = %e, "Gate tier overrides ignored");
        aura_agent_gate::Tiers::for_frontier(&frontier)
    });
    let edits = runner.settings().policy.coordinator_edits;
    config.system_prompt.push_str("\n\n");
    config
        .system_prompt
        .push_str(&prompts::coordinator_section(&tiers, edits));
    aura_agent_gate::coordinator_tools(tools, edits)
}

/// Run one gate worker on `kernel` (a fresh child agent) with the
/// surface's base loop `config` and `tools`, narrowed by `request`.
pub async fn run_worker(
    kernel: Arc<Kernel>,
    mut config: AgentLoopConfig,
    tools: Vec<ToolDefinition>,
    request: &SubagentDispatchRequest,
    cancel: CancellationToken,
) -> SubagentResult {
    let child = kernel.agent_id;
    if let Some(model) = &request.model_override {
        config.max_context_tokens =
            Some(aura_engine::model_context::context_window_for_model(model));
        config.model.clone_from(model);
    }
    config.user_thinking_effort = request
        .reasoning_effort_override
        .as_deref()
        .and_then(ThinkingEffort::from_wire);
    if let Some(addendum) = &request.system_prompt_addendum {
        config.system_prompt.push_str("\n\n");
        config.system_prompt.push_str(addendum);
    }
    let mut timeout = None;
    if let Some(budget) = &request.override_budget {
        config.max_iterations = budget.max_iterations as usize;
        if let Some(max_tokens) = budget.max_tokens {
            config.max_tokens = max_tokens;
        }
        timeout = Some(Duration::from_millis(budget.timeout_ms));
    }
    config.observers.clear();
    let tools: Vec<ToolDefinition> = match &request.override_tool_subset {
        Some(names) => tools.into_iter().filter(|t| names.contains(&t.name)).collect(),
        None => tools,
    };
    let model = config.model.clone();

    if let Err(e) = kernel
        .process_direct(Transaction::user_prompt(child, request.prompt.clone()))
        .await
    {
        return failed(child, format!("could not record the block prompt: {e}"));
    }
    let model_gateway = KernelModelGateway::new(Arc::clone(&kernel));
    let tool_gateway = KernelToolGateway::new(kernel);
    let agent_loop = AgentLoop::new(config);
    let run = agent_loop.run_with_events(
        &model_gateway,
        &tool_gateway,
        vec![Message::user(request.prompt.clone())],
        tools,
        None,
        Some(cancel.clone()),
    );
    let outcome = match timeout {
        Some(limit) => match tokio::time::timeout(limit, run).await {
            Ok(outcome) => outcome,
            Err(_) => {
                cancel.cancel();
                let mut result = failed(child, String::new());
                result.exit = SubagentExit::Timeout;
                return result;
            }
        },
        None => run.await,
    };
    let result = match outcome {
        Ok(result) => result,
        Err(e) => return failed(child, e.to_string()),
    };
    let usage = Usage {
        input_tokens: result.total_input_tokens,
        output_tokens: result.total_output_tokens,
        cache_creation_input_tokens: Some(result.total_cache_creation_input_tokens),
        cache_read_input_tokens: Some(result.total_cache_read_input_tokens),
    };
    let exit = if cancel.is_cancelled() {
        SubagentExit::Cancelled
    } else if result.timed_out {
        SubagentExit::Timeout
    } else if let Some(error) = result.llm_error.clone() {
        SubagentExit::Failed { reason: error }
    } else {
        SubagentExit::Completed
    };
    SubagentResult {
        child_agent_id: Some(child),
        final_message: result.total_text.trim().to_string(),
        total_input_tokens: result.total_input_tokens,
        total_output_tokens: result.total_output_tokens,
        files_changed: result.file_changes.into_iter().map(|c| c.path).collect(),
        exit,
        cost_micros: model_catalog::estimate_cost_usd(&model, &usage).map(usd_to_micros),
    }
}

fn failed(child: AgentId, reason: String) -> SubagentResult {
    let mut result = SubagentResult::completed(child, String::new());
    result.exit = SubagentExit::Failed { reason };
    result
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn usd_to_micros(usd: f64) -> u64 {
    (usd.max(0.0) * 1_000_000.0).round() as u64
}
