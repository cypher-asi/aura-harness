//! Context management: compaction, checkpoints, and budget warnings.

use aura_context_compaction::{
    self as compaction, CompactionAction, CompactionInput, CompactionPolicy, SummaryInput,
    SummaryOutput,
};
use aura_model::{ModelRequestKind, ToolDefinition};
use tokio::sync::mpsc::Sender;

use crate::budget;
use crate::dup_audit;
use crate::events::AgentLoopEvent;
use crate::helpers;
use crate::sanitize;
use crate::types::AgentContextBreakdown;
use aura_config::CHARS_PER_TOKEN;

use super::context_contents;
use super::decisions;
use super::streaming;
use super::{AgentLoopConfig, LoopState};

#[derive(Debug)]
pub(super) enum CompactionOutcome {
    None,
    Applied(compaction::CompactionConfig),
    NeedsSummary(SummaryInput),
}

/// Operator kill switch: when
/// `aura_config::agent().compaction.disabled` is `true` (sourced from
/// `AURA_AGENT_DISABLE_COMPACTION` once at startup), every compaction
/// entry point in the agent loop becomes a no-op. Used to test
/// whether read-spiral behavior is being driven by older `read_file`
/// results getting truncated out of context.
#[must_use]
pub(super) fn compaction_disabled_by_env() -> bool {
    aura_config::agent().compaction.disabled
}

/// Return the [`ModelRequestKind`] the compaction policy should reason
/// about for the upcoming model call at `iteration`.
///
/// [`AgentLoopConfig::request_kind`] is set once at task start (the
/// dev-loop seeds it to [`ModelRequestKind::DevLoopBootstrap`] in
/// `configure_loop_config`) and is never mutated for the life of the
/// task. [`LoopState::build_request`] dynamically swaps the *wire*
/// request kind to [`ModelRequestKind::DevLoopContinuation`] after
/// iteration 0, but the compaction policy reads the stale config field
/// directly. Bootstrap carries a 24 KiB body cap
/// (`DEV_LOOP_BOOTSTRAP_TOTAL_TEXT_MAX_BYTES`); continuation has none.
/// Without this helper, [`aura_context_compaction::effective_pressure`] keeps
/// applying the bootstrap cap to every continuation iteration and
/// `cap_pressure` clamps to 1.0 as soon as `state.messages` crosses
/// ~24 KiB — which fires `NeedsSummary` on every iteration regardless
/// of actual context utilisation, doubling the outbound API call rate
/// for the rest of the task.
///
/// The match here mirrors the bootstrap → continuation transition
/// [`LoopState::build_request`] already performs (see the
/// `(_, _, DevLoopBootstrap, _) => DevLoopContinuation` arm) so the
/// compaction policy sees the same kind the next wire request will
/// carry. Non-dev-loop kinds (`Chat`, explicit `Auxiliary`, the
/// project-tool kinds) pass through unchanged because they either
/// have no body cap or already carry the correct kind statically.
#[must_use]
pub(super) fn effective_compaction_request_kind(
    config: &AgentLoopConfig,
    iteration: usize,
) -> ModelRequestKind {
    match (config.request_kind, iteration) {
        (ModelRequestKind::DevLoopBootstrap, 0) => ModelRequestKind::DevLoopBootstrap,
        (ModelRequestKind::DevLoopBootstrap, _) => ModelRequestKind::DevLoopContinuation,
        (kind, _) => kind,
    }
}

/// Room compaction keeps free in the context window for the next reply.
/// A reply may run longer: the request asks for all that is left (see
/// [`room_left_for_reply`]).
const REPLY_HEADROOM_TOKENS: u64 = 16_384;

fn reserved_output_tokens(config: &AgentLoopConfig, max_ctx: u64) -> u64 {
    u64::from(aura_model::model_catalog::max_output_tokens(&config.model))
        .min(REPLY_HEADROOM_TOKENS)
        .min(max_ctx)
}

/// The most the next reply may take of the context window, or `None` when
/// the window is unknown or less than [`REPLY_HEADROOM_TOKENS`] is left.
/// The API refuses a request whose input and `max_tokens` together exceed
/// the window. With less than the headroom left the request is sent
/// anyway: its refusal is an overflow, which compacts the conversation.
/// The system prompt and tools are counted on top of the estimate, which
/// may already hold them, so the room errs small.
pub(super) fn room_left_for_reply(
    config: &AgentLoopConfig,
    state: &LoopState,
    tools: &[ToolDefinition],
) -> Option<u32> {
    let window = config.max_context_tokens?;
    let prompt = chars_to_tokens(config.system_prompt.len() + compaction::tools_chars(tools));
    let room = window.saturating_sub(current_context_tokens(state).saturating_add(prompt));
    (room >= REPLY_HEADROOM_TOKENS).then(|| u32::try_from(room).unwrap_or(u32::MAX))
}

#[cfg(test)]
fn compaction_pressure_tokens(
    config: &AgentLoopConfig,
    estimated_tokens: u64,
    max_ctx: u64,
) -> u64 {
    estimated_tokens
        .saturating_add(reserved_output_tokens(config, max_ctx))
        .min(max_ctx)
}

fn heuristic_context_tokens(messages: &super::state::History) -> u64 {
    #[allow(clippy::cast_possible_truncation)]
    {
        (messages.chars() / CHARS_PER_TOKEN) as u64
    }
}

fn current_context_tokens(state: &LoopState) -> u64 {
    state
        .last_context_tokens_estimate
        .unwrap_or_default()
        .max(heuristic_context_tokens(&state.messages))
}

/// Char-to-token conversion shared by every per-bucket estimate. Wraps
/// the existing `chars / CHARS_PER_TOKEN` heuristic so the breakdown
/// stays directly comparable to [`AgentLoopResult::estimated_context_tokens`].
fn chars_to_tokens(chars: usize) -> u64 {
    #[allow(clippy::cast_possible_truncation)]
    {
        (chars / CHARS_PER_TOKEN) as u64
    }
}

/// Recompute every per-bucket token estimate from the current loop
/// state. Called after every compaction step (and after overflow
/// recovery) so the value stays in sync with `estimated_context_tokens`.
///
/// Callers pass the *effective* tool surface (post intent-classifier /
/// tool-hints filtering) when available, so the bucket reflects what
/// the model actually receives on the next turn rather than the raw
/// configured surface.
///
/// `system_prompt_tokens` is reported net of [`AgentLoopConfig::skills_chars`]
/// because the runtime injects skill summaries directly into the
/// system prompt; without the subtraction those chars would be
/// double-counted (once under "System prompt" and once under
/// "Skills") and the stacked-bar breakdown in the UI would always
/// look fuller than `estimated_context_tokens`.
fn recompute_breakdown(
    config: &AgentLoopConfig,
    state: &mut LoopState,
    effective_tools: &[ToolDefinition],
) {
    let system_prompt_chars = config
        .system_prompt
        .len()
        .saturating_sub(config.skills_chars);
    state.result.context_breakdown = AgentContextBreakdown {
        system_prompt_tokens: chars_to_tokens(system_prompt_chars),
        tools_tokens: chars_to_tokens(compaction::tools_chars(effective_tools)),
        skills_tokens: chars_to_tokens(config.skills_chars),
        mcp_tokens: 0,
        subagents_tokens: chars_to_tokens(config.subagents_chars),
        conversation_tokens: heuristic_context_tokens(&state.messages),
        // Cache hit/miss numbers come from `accumulate_response`
        // after each model reply; `recompute_breakdown` runs
        // pre-call and has no usage data yet, so default to 0.
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
    };
    state.result.context_contents =
        context_contents::build_context_contents(config, effective_tools);
}

/// Sanitize messages and apply compaction if context utilization is high.
///
/// Returns the compaction outcome so the async loop can perform model-backed
/// summary escalation outside the pure compaction crate.
///
/// `iteration` is the 0-based index of the upcoming model call.
/// [`effective_compaction_request_kind`] uses it to project the stale
/// [`AgentLoopConfig::request_kind`] forward through the bootstrap →
/// continuation transition so the compaction policy doesn't keep
/// applying the bootstrap body cap to every post-bootstrap turn.
#[allow(clippy::cast_precision_loss)]
pub(super) fn compact_if_needed(
    config: &AgentLoopConfig,
    state: &mut LoopState,
    tools: &[ToolDefinition],
    iteration: usize,
) -> CompactionOutcome {
    sanitize::validate_and_repair(&mut state.messages);

    if compaction_disabled_by_env() {
        recompute_breakdown(config, state, tools);
        return CompactionOutcome::None;
    }

    let Some(max_ctx) = config.max_context_tokens else {
        recompute_breakdown(config, state, tools);
        return CompactionOutcome::None;
    };

    let estimated_tokens = current_context_tokens(state);
    state.result.estimated_context_tokens = estimated_tokens;
    let reserved_tokens = reserved_output_tokens(config, max_ctx);
    let raw_message_bytes = state.messages.chars();
    let request_kind = effective_compaction_request_kind(config, iteration);

    // Phase 8: fire `PreCompact` hook. A `Block` decision skips
    // compaction this turn. Empty-engine short-circuit guarantees
    // zero overhead for empty installs.
    if let Some(host) = config.plugin_hooks.as_ref() {
        if !host.is_empty(aura_plugin_hooks::HookEvent::PreCompact) {
            let outcome = host.fire_pre_compact(estimated_tokens, "auto");
            if outcome.is_blocked() {
                tracing::info!("PreCompact hook blocked compaction this turn");
                recompute_breakdown(config, state, tools);
                return CompactionOutcome::None;
            }
        }
    }

    let input = CompactionInput {
        messages: &mut state.messages,
        policy: CompactionPolicy {
            current_context_tokens: Some(estimated_tokens),
            raw_message_bytes: Some(raw_message_bytes),
            request_kind: Some(request_kind),
            ..CompactionPolicy::new(Some(max_ctx), estimated_tokens, reserved_tokens)
        },
    };
    if let Some(trigger) = config.compaction_trigger {
        if compaction::effective_pressure(&input) < trigger {
            recompute_breakdown(config, state, tools);
            return CompactionOutcome::None;
        }
    }
    let report = compaction::compact_messages(input);
    let outcome = match report.action {
        CompactionAction::Applied(tier) => CompactionOutcome::Applied(tier),
        CompactionAction::NeedsSummary(input) => CompactionOutcome::NeedsSummary(input),
        CompactionAction::None => CompactionOutcome::None,
    };

    if !matches!(outcome, CompactionOutcome::None) {
        dup_audit::audit_tool_result_duplicates(&state.messages, "compact_if_needed.post_splice");
        sanitize::validate_and_repair(&mut state.messages);
        let compacted_tokens = heuristic_context_tokens(&state.messages);
        state.last_context_tokens_estimate = Some(compacted_tokens);
        state.result.estimated_context_tokens = compacted_tokens;
        let tier = match &outcome {
            CompactionOutcome::Applied(tier) => format!("{tier:?}"),
            _ => "summary_requested".to_string(),
        };
        state.decide(decisions::compaction(
            tier,
            estimated_tokens,
            compacted_tokens,
            max_ctx,
        ));

        // Phase 8: fire `PostCompact` hook (observer-only).
        if let Some(host) = config.plugin_hooks.as_ref() {
            if !host.is_empty(aura_plugin_hooks::HookEvent::PostCompact) {
                let summary = match &outcome {
                    CompactionOutcome::Applied(tier) => format!("{tier:?}"),
                    CompactionOutcome::NeedsSummary(_) => "needs_summary".to_string(),
                    CompactionOutcome::None => "none".to_string(),
                };
                host.fire_post_compact(estimated_tokens, compacted_tokens, &summary);
            }
        }
    }

    recompute_breakdown(config, state, tools);
    outcome
}

pub(super) fn apply_summary_output(
    config: &AgentLoopConfig,
    state: &mut LoopState,
    tools: &[ToolDefinition],
    summary: SummaryOutput,
) -> bool {
    if compaction_disabled_by_env() {
        recompute_breakdown(config, state, tools);
        return false;
    }
    let before_tokens = current_context_tokens(state);
    let report = compaction::Compactor::new().apply_summary(&mut state.messages, summary);
    if !report.reduced() {
        recompute_breakdown(config, state, tools);
        return false;
    }

    dup_audit::audit_tool_result_duplicates(&state.messages, "apply_summary_output.post_splice");
    sanitize::validate_and_repair(&mut state.messages);
    let compacted_tokens = heuristic_context_tokens(&state.messages);
    state.last_context_tokens_estimate = Some(compacted_tokens);
    state.result.estimated_context_tokens = compacted_tokens;
    state.decide(decisions::compaction(
        "summary",
        before_tokens,
        compacted_tokens,
        config.max_context_tokens.unwrap_or_default(),
    ));
    recompute_breakdown(config, state, tools);
    true
}

pub(super) const FIRST_WRITE_CHECKPOINT: &str =
    "NOTE: You've made your first file change. Before making more changes, \
     consider verifying your work (e.g., run the build or tests) to catch \
     issues early.";

/// Append the first-write checkpoint note to the conversation once.
///
/// Model-facing only: it is not emitted as a UI warning.
pub(super) fn emit_checkpoint_if_needed(state: &mut LoopState) {
    if !state.had_any_write || state.checkpoint_emitted {
        return;
    }
    state.checkpoint_emitted = true;
    helpers::append_warning(&mut state.messages, FIRST_WRITE_CHECKPOINT);
}

/// Check and emit budget warnings.
///
/// In unlimited-iteration mode (`max_iterations == usize::MAX`), the
/// iteration-utilization warnings are skipped — utilization would
/// round to ~0 and the warnings would never fire anyway, but the
/// short-circuit makes the intent explicit and avoids any cast-related
/// precision surprises. The exploration "approaching limit" warning
/// was removed by the cook-loop-fix strip (2026-05) along with the
/// hard exploration cap.
#[allow(clippy::cast_precision_loss)]
pub(super) fn check_budget_warnings(
    config: &AgentLoopConfig,
    event_tx: Option<&Sender<AgentLoopEvent>>,
    state: &mut LoopState,
    iteration: usize,
) {
    if config.max_iterations != usize::MAX {
        let utilization = (iteration + 1) as f64 / config.max_iterations as f64;
        if let Some(warning) =
            budget::check_budget_warning(&mut state.budget_state, utilization, state.had_any_write)
        {
            let decision = decisions::budget_warning(state, utilization, &warning);
            state.decide(decision);
            helpers::append_warning(&mut state.messages, &warning);
            streaming::emit(event_tx, AgentLoopEvent::Warning(warning));
        }
    }
}

/// Check whether the loop should stop due to budget exhaustion.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
pub(super) fn should_stop_for_budget(
    config: &AgentLoopConfig,
    state: &LoopState,
    iteration: usize,
) -> bool {
    let total_tokens = state.result.total_input_tokens + state.result.total_output_tokens;
    let iterations_done = (iteration as u64) + 1;
    let avg_tokens = total_tokens / iterations_done.max(1);
    budget::should_stop_for_budget(
        iteration + 1,
        config.max_iterations,
        avg_tokens,
        total_tokens,
        config.credit_budget,
    )
}

#[cfg(test)]
mod tests;
