//! # aura-core-types
//!
//! Layer: core
//!
//! Behavior-free IDs and lightweight shared shapes for the new
//! layered architecture.
//!
//! ## Phase 1 scope
//!
//! Phase 1 lands the following NEW identifier newtypes here:
//! [`TurnId`], [`RunId`], [`ToolCallId`], [`UserId`], [`SessionId`],
//! and [`TransactionId`].
//!
//! The legacy identifiers (`AgentId`, `Hash`, `ActionId`,
//! `ProcessId`, etc.) and the larger domain types
//! (`Transaction`, `Action`, `Effect`, `RecordEntry`,
//! `SpawnSpec`, `SubagentResult`, …) still live in `aura-core` and
//! are re-exported through it. They migrate here in Phase 1.5 / Phase
//! 2 as the rest of the workspace is rewired off `aura-core`
//! directly. Re-exporting in the other direction in Phase 1 would
//! create a dependency cycle because `aura-core` consumes the
//! permissions and modes crates.
//!
//! ## Invariants
//!
//! - Every id is a fixed-size byte newtype with hex display.
//! - `from_uuid` and `generate` always produce deterministic byte
//!   representations.
//!
//! ## Failure modes
//!
//! - [`hex::FromHexError`] when `from_hex` is called with malformed
//!   input.

#![forbid(unsafe_code)]

mod error;
pub mod hash;
mod ids;
mod registry;
mod serde_helpers;
pub mod text;
pub mod time;
mod types;

pub use ids::{RunId, SessionId, ToolCallId, TransactionId, TurnId, UserId};

pub use error::{AuraError, ErrorChain, Result};
pub use ids::{
    ActionId, AgentEventId, AgentId, FactId, Hash, ProcedureId, ProcessId, SPAWN_ZNS_PREFIX,
};
pub use registry::{Registry, RegistryError};
pub use types::trace;
pub use types::{
    installed_integrations_satisfy, integration_match, Action, ActionKind, ActionResultPayload,
    AgentStatus, ArmSpec, Assessment, BenchSource, CacheControl, CompareArm, CompareCopy,
    CompareJudgment, ComparePayload, CompareRun, CompareStatus, ContextHash, Decision, Difficulty,
    Effect, EffectKind, EffectStatus, ExampleTaskInfo, Exploration, Identity,
    InstalledIntegrationDefinition, InstalledToolCapability, InstalledToolDefinition,
    InstalledToolIntegrationRequirement, InstalledToolRuntimeAuth, InstalledToolRuntimeExecution,
    InstalledToolRuntimeIntegration, InstalledToolRuntimeProviderExecution, LineDiff,
    ModeChangePayload, ModelTokens, OutcomePayload, OutcomeSignal, Pitfall, Plan, PlanShare,
    PlanUsage, PolicyApproval, PolicyApprover, PolicyCheck, PolicyCheckOutcome, PolicyDecision,
    PolicyRule, PolicySource, ProcessPending, PromptCategory, PromptPayload, Proposal, ProposalSet,
    RecordEntry, RecordEntryBuilder, RefVerdict, Reference, RejectedProposal, Route, RouteLevel,
    RouteReason, RunTurn, RuntimeCapabilityInstall, Scope, SessionBaseline, Spec, SubagentBudget,
    SubagentDispatchRequest, SubagentExit, SubagentKindSpec, SubagentResult, SystemKind, ToolAuth,
    ToolCall, ToolCallContext, ToolDefinition, ToolExecution, ToolGateVerdict, ToolProposal,
    ToolResult, ToolResultContent, ToolResultImage, ToolResultKind, Trace, Transaction,
    TransactionType, DEFAULT_SUBAGENT_TIMEOUT_MS, KERNEL_VERSION, MAX_TURNS,
};
pub use types::{
    Placement, PlacementPayload, PriceRates, VmEvent, VmPayload, VmResources, VmSource, VmUsage,
};

// Convenience re-exports of mode/permission primitives so downstream
// crates can pull from one place once they're migrated. These do not
// create cycles because aura-core-types depends on both.
pub use aura_core_modes::{
    AgentMode, AgentRole, ApprovalLevel, CapabilityProfile, DefaultModeGate, GatedAction,
    KernelMode, ModeGate, ModeProfile, ModeViolation, ReplayMode, SandboxMode, SpawnMode,
};
pub use aura_core_permissions::{
    allows, allows_tool, effective, intersect, is_effectively_full_access, narrow,
    resolve_effective_permission, resolve_effective_permission_with_source, AgentPermissions,
    AgentScope, AgentToolPermissions, Capability, EffectivePermissions, GrantSource,
    PermissionDecision, PermissionError, PermissionLayer, Permissions, PrivilegeGrant, ToolState,
    UserDefaultMode, UserToolDefaults,
};
