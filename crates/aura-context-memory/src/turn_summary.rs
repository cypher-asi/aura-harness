//! Layer-neutral summary of a single completed agent turn.
//!
//! Phase 6c (context-memory inversion): the memory pipeline used to read
//! `aura_agent::AgentLoopResult` directly. That produced an upward edge
//! `aura-context-memory -> aura-agent` which violated the layer order.
//!
//! [`TurnSummary`] mirrors only the fields the memory pipeline actually
//! consumes (heuristic extraction + write reporting), keeping the
//! context-layer crate free of any agent-layer types. The conversion
//! from `AgentLoopResult` to `TurnSummary` lives one layer up in
//! `aura-runtime::memory_observer` where both crates are in scope.
//!
//! # Invariants
//!
//! - The shape is intentionally a strict subset of `AgentLoopResult`.
//!   Adding a field here means the runtime-side `turn_summary_from_result`
//!   adapter must also grow a member-by-member copy of it.
//! - `messages` is cloned by the adapter because the pipeline derives a
//!   [`crate::extraction::ConversationTurn`] from the slice after the turn
//!   has settled — the agent-loop's owning `AgentLoopResult` is no longer
//!   reachable by then.

/// Layer-neutral mirror of `aura_agent::AgentLoopResult` for the memory
/// pipeline.
///
/// Only the fields the memory subsystem reads are surfaced. Documented
/// per-field to keep the contract explicit: anything the heuristic
/// extractor, the LLM refiner, or the write pipeline observes must appear
/// here, and the runtime-side adapter must populate it from the live
/// `AgentLoopResult`.
#[derive(Debug, Clone, Default)]
pub struct TurnSummary {
    /// Whether the turn was cancelled or hit its wall-clock limit.
    ///
    /// Drives the `timed_out` outcome label in
    /// [`crate::extraction::HeuristicExtractor::extract_task_outcome`]; a
    /// timed-out turn still produces a task-outcome event so the agent's
    /// memory reflects the failure mode.
    pub timed_out: bool,

    /// Whether the loop self-aborted due to stall detection (no progress
    /// across the configured iteration window).
    ///
    /// Drives the `stalled` outcome label in the heuristic extractor's
    /// task-outcome record. Mutually exclusive with `timed_out` /
    /// `llm_error` per the original loop semantics, but the extractor
    /// gates on a deterministic priority order so concurrent flags
    /// behave predictably.
    pub stalled: bool,

    /// Provider-side error string that terminated the loop, if any.
    ///
    /// Drives the `llm_error` outcome label in the heuristic extractor.
    /// Stored as `Option<String>` rather than a typed error because the
    /// memory pipeline never re-classifies it — it only records that an
    /// error happened, not what kind.
    pub llm_error: Option<String>,

    /// Accumulated assistant text across all iterations of the turn.
    ///
    /// Used by [`crate::extraction::HeuristicExtractor::extract_from_text`]
    /// for keyword scanning (e.g. "the project uses ...") and by
    /// [`crate::extraction::ConversationTurn::from_messages`] as the
    /// fallback assistant text when message replay alone is empty.
    pub total_text: String,

    /// Total input tokens billed for the turn.
    ///
    /// Surfaced into the task-outcome event metadata so memory analytics
    /// can correlate retention/forgetting decisions against per-turn
    /// spend.
    pub total_input_tokens: u64,

    /// Total output tokens billed for the turn.
    ///
    /// Same rationale as `total_input_tokens`: tracked in the
    /// task-outcome event for downstream analytics.
    pub total_output_tokens: u64,

    /// Number of model iterations executed in the turn.
    ///
    /// The heuristic extractor short-circuits when this is zero (no
    /// model call happened, so there is nothing worth remembering).
    pub iterations: usize,

    /// Final message history at the end of the turn.
    ///
    /// `aura_model_reasoner::Message` is a context-layer-friendly dependency
    /// (the reasoner sits below `context` in the layer order). The
    /// refiner builds a [`crate::extraction::ConversationTurn`] from
    /// these messages, walking backward to recover the last
    /// user/assistant exchange for the LLM extraction prompt.
    pub messages: Vec<aura_model_reasoner::Message>,
}
