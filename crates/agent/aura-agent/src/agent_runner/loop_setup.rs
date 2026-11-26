//! Loop configuration, task-context construction, and loop-result mapping.

use std::time::Duration;

use aura_model::ModelRequestKind;

use aura_context_prompts::bootstrap::{build_agentic_task_context, BootstrapCtx};

use super::{AgentRunnerConfig, AgenticTaskParams, TaskExecutionResult};
use crate::agent_loop::AgentLoopConfig;
use crate::prompt_resolve::{default_caps, extract_hints, resolve_hints, FsWorkspace};
use crate::task_context;
use crate::turn_config::{resolve_simple_model, TaskComplexity};
use crate::types::AgentLoopResult;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build an [`AgentLoopConfig`] from task complexity and runner config.
#[must_use]
pub fn configure_loop_config(
    complexity: TaskComplexity,
    config: &AgentRunnerConfig,
    _member_count: usize,
    system_prompt: String,
) -> AgentLoopConfig {
    let max_iterations = match complexity {
        TaskComplexity::Simple => config.max_agentic_iterations.min(15),
        _ => config.max_agentic_iterations,
    };
    let model = match complexity {
        TaskComplexity::Simple => resolve_simple_model(&config.simple_model),
        _ => config.default_model.clone(),
    };

    // Phase 5: the `early_test_oracle` field on the returned
    // `AgentLoopConfig` is populated later inside `execute_task_inner`
    // because that's where `params.project.test_command` is in
    // scope. The struct field defaults to `None` here.
    AgentLoopConfig {
        max_iterations,
        user_thinking_effort: config.reasoning_effort,
        stream_timeout: Duration::from_secs(config.stream_timeout_secs),
        billing_reason: "aura_task".to_string(),
        max_context_tokens: Some(config.max_context_tokens),
        credit_budget: config.max_task_credits,
        auto_build_cooldown: 1,
        auth_token: config.auth_token.clone(),
        system_prompt,
        // Forward the router/billing identifiers populated by the
        // automaton bridge so outbound `/v1/messages` calls carry
        // `X-Aura-Org-Id` / `X-Aura-Session-Id`. Same pattern as the
        // interactive-chat path (`SessionState`) — the loop builder
        // at line ~676 of `agent_loop/mod.rs` calls
        // `.aura_org_id(config.aura_org_id.clone())` /
        // `.aura_session_id(...)` on the `ModelRequest`, so we just
        // need to make sure the `AgentLoopConfig` we hand it is
        // populated.
        aura_org_id: config.aura_org_id.clone(),
        aura_session_id: config.aura_session_id.clone(),
        aura_agent_id: config.aura_agent_id.clone(),
        aura_project_id: config.aura_project_id.clone(),
        prompt_cache_key: config.prompt_cache_key.clone(),
        request_kind: ModelRequestKind::DevLoopBootstrap,
        // Temporary (2026-05): the dev-loop policy pins reasoning
        // effort to `Medium` across every iteration. This flag is
        // the "this is a dev-loop run" signal that
        // `LoopState::compute_thinking_effort` reads to bypass the
        // codex-style `Off → Medium → Low` taper still used by chat
        // and other generic callers. The historical iteration-0
        // `max_tokens` clamp that used to ride on this flag has been
        // removed (see `AgentLoopConfig::disable_thinking_iteration_0`).
        disable_thinking_iteration_0: true,
        ..AgentLoopConfig::for_agent(model)
    }
}

/// Build the task-context bundle that backs both the initial user
/// message and the `get_task_context` tool response.
///
/// Stripped (2026-05): historically this returned the full
/// project/spec/task header plus a 160 KB bundle of workspace map,
/// type defs, codebase snapshot, and dep-API surface (capped via
/// `aura_config::MAX_TASK_CONTEXT_CHARS`). On every `get_task_context`
/// call the agent received ~40 K tokens of pre-bundled material; once
/// compaction redacted the older copy the model called the tool again
/// and the conversation thrashed. The agent already has
/// `read_file` / `search_code` / `list_files` — give it the task text
/// and let it pull the rest on demand.
///
/// On `params.attempt == 0` this resolves the Phase 4 enrichment block
/// (paths + symbols extracted from the task description, looked up on
/// disk) and splices it into the returned context. Each per-call IO is
/// wrapped in a 2-second timeout (see [`default_caps`]), so a slow
/// filesystem or grep can NEVER block context construction — the worst
/// case is that the enrichment block is omitted and the agent gets the
/// pre-Phase-4 prompt. On retries the resolve is skipped entirely.
pub(super) async fn build_full_task_ctx(params: &AgenticTaskParams<'_>) -> String {
    let work_log_summary = task_context::build_work_log_summary(params.work_log);
    let enrichment_block = resolve_enrichment_block(params).await;
    build_agentic_task_context(BootstrapCtx {
        project: params.project,
        spec: params.spec,
        task: params.task,
        session: params.session,
        completed_deps: params.completed_deps,
        work_log_summary: &work_log_summary,
        attempt: params.attempt,
        enrichment_block: enrichment_block.as_deref(),
    })
}

/// Phase 4: extract candidate paths & symbols from the task description
/// and resolve them against the project folder. Returns `None` (and
/// emits nothing in the task context) when:
/// - `attempt > 0` (retries skip enrichment — see Phase 5),
/// - the description has no resolvable hints,
/// - the project `folder_path` is empty / nonexistent,
/// - or every resolve call timed out / hit a missing file.
pub(super) async fn resolve_enrichment_block(params: &AgenticTaskParams<'_>) -> Option<String> {
    if params.attempt != 0 {
        return None;
    }
    let text = format!("{} {}", params.task.title, params.task.description);
    let hints = extract_hints(&text);
    if !hints.is_meaningful() {
        return None;
    }
    let folder = params.project.folder_path;
    if folder.is_empty() {
        return None;
    }
    let workspace = FsWorkspace::new(folder);
    let resolved = resolve_hints(&hints, &workspace, default_caps()).await;
    if resolved.is_empty() {
        return None;
    }
    Some(resolved.into_block())
}

/// Process an [`AgentLoopResult`] into a [`TaskExecutionResult`].
pub(super) fn finalize_loop_result(result: AgentLoopResult) -> TaskExecutionResult {
    let AgentLoopResult {
        total_text,
        total_input_tokens,
        total_output_tokens,
        messages,
        ..
    } = result;
    let notes = if total_text.is_empty() {
        "Task completed via agentic tool-use loop".to_string()
    } else {
        total_text
    };
    TaskExecutionResult {
        notes,
        follow_up_tasks: Vec::new(),
        input_tokens: total_input_tokens,
        output_tokens: total_output_tokens,
        files_already_applied: true,
        messages,
    }
}

/// Attach the EarlyTestOracle config only when the per-call flag is on AND
/// the project declares a non-blank `test_command`.
pub(super) fn early_test_oracle_config(
    enabled: bool,
    params: &AgenticTaskParams<'_>,
) -> Option<aura_config::EarlyTestOracleConfig> {
    if enabled {
        let test_command = params
            .project
            .test_command
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty());
        test_command.map(|cmd| aura_config::EarlyTestOracleConfig {
            enabled: true,
            test_command: Some(cmd),
        })
    } else {
        None
    }
}

/// Map an agent loop result that did not complete the task to an error.
pub(super) fn ensure_loop_completed(
    result: &AgentLoopResult,
    require_task_done: bool,
) -> Result<(), crate::AgentError> {
    if let Some(ref llm_err) = result.llm_error {
        return Err(crate::AgentError::Internal(format!("LLM error: {llm_err}")));
    }
    if result.iterations == 0 {
        return Err(crate::AgentError::Internal(
            "Agent loop completed zero iterations — LLM may not be configured correctly".into(),
        ));
    }

    if result.output_truncated || result.timed_out || result.insufficient_credits || result.stalled
    {
        return Err(crate::AgentError::Internal(
            "Task execution stopped before completion".into(),
        ));
    }
    if require_task_done && !result.task_done_completed {
        return Err(crate::AgentError::Internal(
            "Task ended without an accepted task_done completion".into(),
        ));
    }
    Ok(())
}
