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
#![warn(clippy::all)]

pub(crate) mod error;
pub mod hash;
pub(crate) mod ids;
pub(crate) mod permissions;
pub(crate) mod registry;
pub(crate) mod serde_helpers;
pub(crate) mod time;
pub(crate) mod types;

pub use error::{AuraError, Result};
#[allow(deprecated)]
pub use ids::{ActionId, AgentEventId, AgentId, FactId, Hash, ProcedureId, ProcessId, TxId};
pub use permissions::{AgentPermissions, AgentScope, Capability};
pub use registry::{Registry, RegistryError};
pub use types::{
    resolve_effective_permission, Action, ActionKind, ActionResultPayload, AgentStatus,
    AgentToolPermissions, CacheControl, ContextHash, Decision, Effect, EffectKind, EffectStatus,
    Identity, InstalledIntegrationDefinition, InstalledToolCapability, InstalledToolDefinition,
    InstalledToolIntegrationRequirement, InstalledToolRuntimeAuth, InstalledToolRuntimeExecution,
    InstalledToolRuntimeIntegration, InstalledToolRuntimeProviderExecution, PermissionLevel,
    ProcessPending, Proposal, ProposalSet, RecordEntry, RejectedProposal, RuntimeCapabilityInstall,
    SystemKind, ToolAuth, ToolCall, ToolCallContext, ToolDecision, ToolDefinition, ToolExecution,
    ToolProposal, ToolResult, ToolResultContent, ToolState, Trace, Transaction, TransactionType,
    UserDefaultMode, UserToolDefaults,
};
