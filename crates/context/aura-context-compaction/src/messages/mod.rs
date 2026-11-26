//! Message-history compaction and summarization helpers.

use aura_model::Message;
use tracing::debug;

mod blocks;
mod config;
mod dedup;
mod pressure;
mod redaction;
mod size;
mod storage;
mod summary;

pub use blocks::{compact_older_messages, truncate_content};
pub use config::{
    CompactionAction, CompactionConfig, CompactionInput, CompactionPolicy, CompactionReport,
    OverflowStep,
};
pub use dedup::{dedup_read_results_by_content_hash, supersede_older_reads};
pub use pressure::{effective_pressure, pick_stricter_tier, select_tier};
pub use redaction::{apply_summary, summarize_write_input, RedactionMarker, ToolSummaryInput};
pub(crate) use size::json_len;
pub use size::{estimate_message_chars, message_chars_to_tokens};
pub use storage::{
    compact_for_storage, truncate_messages_for_storage, SESSION_TOOL_BLOB_MAX_BYTES,
};
pub use summary::{apply_message_summary, forced_summary_input, SummaryInput, SummaryOutput};

use pressure::tier_for;
use summary::build_summary_input;

/// Choose and apply a compaction tier using context-window utilization
/// and explicit request-body caps.
#[allow(clippy::needless_pass_by_value)]
pub fn compact_messages(input: CompactionInput<'_>) -> CompactionReport {
    let before_chars = estimate_message_chars(input.messages);
    let pressure = effective_pressure(&input);
    let chosen = tier_for(pressure);
    let policy = input.policy;

    if let Some(tier) = chosen {
        debug!(
            pressure,
            tool_result_max_chars = tier.tool_result_max_chars,
            text_max_chars = tier.text_max_chars,
            preserve_recent = tier.preserve_recent,
            "Compacting context"
        );
        // Phase 2: fold older read-only tool results that the model
        // has already re-read more recently. Runs before
        // `compact_older_messages` so the truncation step never sees
        // (and therefore never re-redacts) an already-folded marker.
        let folded = dedup_read_results_by_content_hash(input.messages);
        if folded > 0 {
            debug!(
                folded,
                "content_hash dedup folded older read-only tool results"
            );
        }
        let superseded = supersede_older_reads(input.messages);
        if superseded > 0 {
            debug!(
                superseded,
                "folded read_file results a later read supersedes"
            );
        }
        compact_older_messages(input.messages, &tier);
    }

    let after_chars = estimate_message_chars(input.messages);
    let action = if pressure >= policy.summary_at {
        build_summary_input(input.messages, policy, before_chars, after_chars).map_or_else(
            || chosen.map_or(CompactionAction::None, CompactionAction::Applied),
            CompactionAction::NeedsSummary,
        )
    } else {
        chosen.map_or(CompactionAction::None, CompactionAction::Applied)
    };

    CompactionReport {
        before_chars,
        after_chars,
        action,
    }
}

/// Apply a specific overflow-recovery tier to messages.
pub fn recover_overflow(messages: &mut [Message], tier: CompactionConfig) -> CompactionReport {
    let before_chars = estimate_message_chars(messages);
    compact_older_messages(messages, &tier);
    let after_chars = estimate_message_chars(messages);
    CompactionReport {
        before_chars,
        after_chars,
        action: CompactionAction::Applied(tier),
    }
}

#[cfg(test)]
mod tests;
