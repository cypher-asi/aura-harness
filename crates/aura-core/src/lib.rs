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
pub mod hash;
pub mod ids;
pub(crate) mod serde_helpers;
pub mod time;
pub mod types;

pub use error::{AuraError, Result};
#[allow(deprecated)]
pub use ids::{ActionId, AgentEventId, AgentId, FactId, Hash, ProcedureId, ProcessId, TxId};
pub use types::{
    Action, ActionKind, ActionResultPayload, AgentStatus, CacheControl, Decision, Effect,
    EffectKind, EffectStatus, Identity, InstalledIntegrationDefinition, InstalledToolCapability,
    InstalledToolDefinition, InstalledToolIntegrationRequirement, InstalledToolRuntimeAuth,
    InstalledToolRuntimeExecution, InstalledToolRuntimeIntegration,
    InstalledToolRuntimeProviderExecution, ProcessPending, Proposal, ProposalSet, RecordEntry,
    RejectedProposal, RuntimeCapabilityInstall, SystemKind, ToolAuth, ToolCall, ToolCallContext,
    ToolDecision, ToolDefinition, ToolExecution, ToolProposal, ToolResult, ToolResultContent,
    Trace, Transaction, TransactionType,
};
