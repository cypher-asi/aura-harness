//! # aura-core
//!
//! Core types, identifiers, schemas, and serialization for the Aura Swarm.
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
pub mod hash;
pub mod ids;
pub mod types;

pub use error::{AuraError, Result};
pub use ids::{ActionId, AgentId, Hash, ProcessId, TxId};
pub use types::{
    Action, ActionKind, ActionResultPayload, Decision, Effect, EffectKind, EffectStatus, Identity,
    ProcessPending, Proposal, ProposalSet, RecordEntry, RejectedProposal, ToolCall, ToolDecision,
    ToolExecution, ToolProposal, ToolResult, Trace, Transaction, TransactionType,
};

// Legacy alias for backwards compatibility
#[allow(deprecated)]
pub use types::TransactionKind;
