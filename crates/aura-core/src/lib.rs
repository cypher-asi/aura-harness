//! # aura-core
//!
//! Core types, identifiers, schemas, and serialization for Aura.
//!
//! This crate provides:
//! - Strongly-typed identifiers (`AgentId`, `TxId`, `ActionId`, `Hash`, `ProcessId`)
//! - Domain types (`Transaction`, `Action`, `Effect`, `RecordEntry`)
//! - Async process types (`ProcessPending`, `ActionResultPayload`)
//! - Error types
//! - Hashing utilities

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic, clippy::nursery)]

pub mod error;
pub mod executor;
pub mod hash;
pub mod ids;
pub(crate) mod serde_helpers;
pub mod types;

pub use error::{AuraError, Result};
pub use executor::{
    DecodedToolResult, ExecuteContext, ExecuteLimits, Executor, ExecutorError, decode_tool_effect,
};
pub use ids::{ActionId, AgentId, Hash, ProcessId, TxId};
pub use types::{
    Action, ActionKind, ActionResultPayload, AgentStatus, CacheControl, Decision, Effect,
    EffectKind, EffectStatus, Identity, InstalledToolDefinition, ProcessPending, Proposal,
    ProposalSet, RecordEntry, RejectedProposal, ToolAuth, ToolCall, ToolCallContext, ToolDecision,
    ToolDefinition, ToolExecution, ToolProposal, ToolResult, ToolResultContent, Trace, Transaction,
    TransactionType,
};

// ---------------------------------------------------------------------------
// Tool result caching (agent loop / kernel turn processor)
// ---------------------------------------------------------------------------

/// Tools whose successful results can be cached within a single run or turn (read-only).
pub const CACHEABLE_TOOLS: &[&str] = &[
    "read_file",
    "list_files",
    "stat_file",
    "find_files",
    "search_code",
];

// ---------------------------------------------------------------------------
// Default model identifiers
// ---------------------------------------------------------------------------

/// Default frontier model for agent loops and sessions.
pub const DEFAULT_MODEL: &str = "claude-opus-4-6";

/// Fallback model used when the primary model is unavailable.
pub const FALLBACK_MODEL: &str = "claude-sonnet-4-6";

/// Deterministic cache key from tool name and JSON arguments (canonical serialization).
#[must_use]
pub fn tool_result_cache_key(tool_name: &str, input: &serde_json::Value) -> String {
    let canonical = serde_json::to_string(input).unwrap_or_else(|_| format!("{input:?}"));
    format!("{tool_name}\0{canonical}")
}

