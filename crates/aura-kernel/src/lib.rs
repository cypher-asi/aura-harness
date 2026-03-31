//! # aura-kernel
//!
//! Deterministic kernel for Aura.
//!
//! This crate provides:
//! - Single-step kernel processing (Spec-01 legacy)
//! - Policy engine for authorization
//! - Context building for model requests
//!
//! ## Architecture
//!
//! The kernel is the deterministic core of AURA. It:
//! 1. Builds context from the record window
//! 2. Calls the model provider for completions
//! 3. Applies policy to authorize actions
//! 4. Executes actions via the executor router
//! 5. Records all inputs/outputs for replay
//!
//! The turn processor and process manager now live under `aura-agent::runtime`.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic, clippy::nursery)]

// TODO(Phase 8): GitExecutor for kernel-recorded git mutations
// TODO(Phase 8): BuildVerifyExecutor for kernel-recorded build/test

mod context;
pub mod executor;
mod kernel;
mod policy;
pub mod router;

pub use context::{Context, ContextBuilder};
pub use executor::{
    decode_tool_effect, DecodedToolResult, ExecuteContext, ExecuteLimits, Executor, ExecutorError,
};
pub use kernel::{Kernel, KernelConfig, ProcessResult, ReasonResult, ReasonStreamHandle, ToolOutput};
pub use policy::{default_tool_permission, PermissionLevel, Policy, PolicyConfig, PolicyResult};
pub use router::ExecutorRouter;

/// Errors from the deterministic kernel (store, reasoner, serialization).
#[derive(Debug, thiserror::Error)]
pub enum KernelError {
    #[error("store error: {0}")]
    Store(String),
    #[error("reasoner error: {0}")]
    Reasoner(String),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("{0}")]
    Internal(String),
}

// Re-export ToolResultContent for convenience
pub use aura_core::ToolResultContent;
