//! Context management: compaction, checkpoints, and budget warnings.

use tokio::sync::mpsc::Sender;
use tracing::debug;

use crate::budget;
use crate::compaction;
use crate::constants::CHARS_PER_TOKEN;
use crate::events::AgentLoopEvent;
use crate::helpers;
use crate::sanitize;

use super::streaming;
use super::{AgentLoopConfig, LoopState};

fn reserved_output_tokens(config: &AgentLoopConfig, max_ctx: u64) -> u64 {
    u64::from(config.max_tokens).min(max_ctx)
}

fn compaction_pressure_tokens(
    config: &AgentLoopConfig,
    estimated_tokens: u64,
    max_ctx: u64,
) -> u64 {
    estimated_tokens
        .saturating_add(reserved_output_tokens(config, max_ctx))
        .min(max_ctx)
}

fn heuristic_context_tokens(messages: &[aura_reasoner::Message]) -> u64 {
    #[allow(clippy::cast_possible_truncation)]
    {
        (compaction::estimate_message_chars(messages) / CHARS_PER_TOKEN) as u64
    }
}

fn current_context_tokens(state: &LoopState) -> u64 {
    state
        .last_context_tokens_estimate
        .unwrap_or_default()
        .max(heuristic_context_tokens(&state.messages))
}

/// Sanitize messages and apply compaction if context utilization is high.
#[allow(clippy::cast_precision_loss)]
pub(super) fn compact_if_needed(config: &AgentLoopConfig, state: &mut LoopState) {
    sanitize::validate_and_repair(&mut state.messages);

    let Some(max_ctx) = config.max_context_tokens else {
        return;
    };

    let estimated_tokens = current_context_tokens(state);
    state.result.estimated_context_tokens = estimated_tokens;
    let pressure_tokens = compaction_pressure_tokens(config, estimated_tokens, max_ctx);
    let utilization = pressure_tokens as f64 / max_ctx as f64;

    if let Some(tier) = compaction::select_tier(utilization) {
        debug!(utilization, "Compacting context");
        compaction::compact_older_messages(&mut state.messages, &tier);
        sanitize::validate_and_repair(&mut state.messages);
        let compacted_tokens = heuristic_context_tokens(&state.messages);
        state.last_context_tokens_estimate = Some(compacted_tokens);
        state.result.estimated_context_tokens = compacted_tokens;
    }
}

/// Apply a specific compaction tier after a provider rejects the request for
/// being too large. Returns `true` when the prompt was actually reduced.
pub(super) fn compact_for_overflow(
    state: &mut LoopState,
    tier: compaction::CompactionConfig,
) -> bool {
    sanitize::validate_and_repair(&mut state.messages);
    let before_chars = compaction::estimate_message_chars(&state.messages);
    let before_tokens = current_context_tokens(state);

    compaction::compact_older_messages(&mut state.messages, &tier);
    sanitize::validate_and_repair(&mut state.messages);

    let after_chars = compaction::estimate_message_chars(&state.messages);
    let after_tokens = heuristic_context_tokens(&state.messages);
    state.last_context_tokens_estimate = Some(after_tokens);
    state.result.estimated_context_tokens = after_tokens;

    after_chars < before_chars || after_tokens < before_tokens
}

/// Emit the first-write checkpoint warning once.
pub(super) fn emit_checkpoint_if_needed(
    event_tx: Option<&Sender<AgentLoopEvent>>,
    state: &mut LoopState,
) {
    if !state.had_any_write || state.checkpoint_emitted {
        return;
    }
    state.checkpoint_emitted = true;
    let msg = "NOTE: You've made your first file change. Before making more changes, \
               consider verifying your work (e.g., run the build or tests) to catch \
               issues early."
        .to_string();
    helpers::append_warning(&mut state.messages, &msg);
    streaming::emit(event_tx, AgentLoopEvent::Warning(msg));
}

/// Apply proactive compaction when exploration usage is high.
pub(super) fn compact_exploration_if_needed(config: &AgentLoopConfig, state: &mut LoopState) {
    if state.exploration_compaction_done {
        return;
    }
    let threshold = (config.exploration_allowance * 2) / 3;
    if state.exploration_state.count < threshold {
        return;
    }
    if config.max_context_tokens.is_none() {
        return;
    }

    let tier = compaction::CompactionConfig::history();
    compaction::compact_older_messages(&mut state.messages, &tier);
    sanitize::validate_and_repair(&mut state.messages);
    state.exploration_compaction_done = true;
    debug!(
        exploration_count = state.exploration_state.count,
        threshold, "Proactive compaction triggered by exploration usage"
    );
}

/// Check and emit budget and exploration warnings.
#[allow(clippy::cast_precision_loss)]
pub(super) fn check_budget_warnings(
    config: &AgentLoopConfig,
    event_tx: Option<&Sender<AgentLoopEvent>>,
    state: &mut LoopState,
    iteration: usize,
) {
    let utilization = (iteration + 1) as f64 / config.max_iterations as f64;
    if let Some(warning) =
        budget::check_budget_warning(&mut state.budget_state, utilization, state.had_any_write)
    {
        helpers::append_warning(&mut state.messages, &warning);
        streaming::emit(event_tx, AgentLoopEvent::Warning(warning));
    }

    if let Some(warning) = budget::check_exploration_warning(
        &mut state.exploration_state,
        config.exploration_allowance,
    ) {
        helpers::append_warning(&mut state.messages, &warning);
        streaming::emit(event_tx, AgentLoopEvent::Warning(warning));
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
mod tests {
    use super::{
        compact_for_overflow, compaction_pressure_tokens, heuristic_context_tokens,
        reserved_output_tokens,
    };
    use crate::agent_loop::AgentLoopConfig;
    use crate::agent_loop::LoopState;
    use aura_reasoner::Message;

    #[test]
    fn reserves_max_tokens_for_output_headroom() {
        let config = AgentLoopConfig {
            max_tokens: 16_384,
            ..AgentLoopConfig::default()
        };
        assert_eq!(reserved_output_tokens(&config, 200_000), 16_384);
    }

    #[test]
    fn reserve_is_capped_by_context_window() {
        let config = AgentLoopConfig {
            max_tokens: 16_384,
            ..AgentLoopConfig::default()
        };
        assert_eq!(reserved_output_tokens(&config, 8_000), 8_000);
    }

    #[test]
    fn pressure_tokens_include_output_reserve() {
        let config = AgentLoopConfig {
            max_tokens: 20_000,
            ..AgentLoopConfig::default()
        };
        assert_eq!(compaction_pressure_tokens(&config, 60_000, 100_000), 80_000);
    }

    #[test]
    fn overflow_compaction_reports_progress_when_history_shrinks() {
        let mut state = LoopState::new(
            &AgentLoopConfig::default(),
            vec![
                Message::user("intro"),
                Message::assistant("A".repeat(4_000)),
                Message::user("B".repeat(4_000)),
                Message::assistant("C".repeat(4_000)),
                Message::user("latest"),
            ],
        );
        state.last_context_tokens_estimate = Some(heuristic_context_tokens(&state.messages));

        assert!(compact_for_overflow(
            &mut state,
            crate::compaction::CompactionConfig::micro()
        ));
    }

    #[test]
    fn overflow_compaction_reports_no_progress_when_nothing_can_change() {
        let mut state = LoopState::new(&AgentLoopConfig::default(), vec![Message::user("hello")]);
        state.last_context_tokens_estimate = Some(heuristic_context_tokens(&state.messages));

        assert!(!compact_for_overflow(
            &mut state,
            crate::compaction::CompactionConfig::aggressive()
        ));
    }
}
