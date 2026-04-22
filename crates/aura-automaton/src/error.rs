use thiserror::Error;

use crate::builtins::dev_loop::DecompositionHint;

/// Errors from automaton lifecycle operations (install, tick, stop) and runtime management.
#[derive(Debug, Error)]
pub enum AutomatonError {
    #[error("automaton not found: {0}")]
    NotFound(String),

    #[error("automaton already running: {0}")]
    AlreadyRunning(String),

    #[error("automaton stopped: {0}")]
    Stopped(String),

    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("domain API error: {0}")]
    DomainApi(String),

    #[error("agent execution error: {0}")]
    AgentExecution(String),

    /// Task reached the implementing phase but produced no file operations
    /// — likely truncated by `max_tokens` or interrupted. Carries a
    /// structured `DecompositionHint` so the orchestrator can auto-split
    /// the task into a skeleton + fill pair instead of a blind retry.
    ///
    /// The Phase 3 orchestrator in aura-os consumes this variant; callers
    /// that only care about the string surface (e.g. `TaskFailed` event
    /// emission) can continue to use `to_string()`.
    #[error(
        "task reached implementation phase but no file operations completed — needs decomposition (failed_paths={}, last_pending={:?})",
        .hint.failed_paths.len(),
        .hint.last_pending_tool_name,
    )]
    NeedsDecomposition { hint: DecompositionHint },

    #[error("credits exhausted")]
    CreditsExhausted,

    /// Catch-all for unexpected conditions that don't fit a typed variant.
    /// Prefer adding a dedicated variant over introducing new call-sites here.
    #[error("unexpected: {0}")]
    Unexpected(String),
}
