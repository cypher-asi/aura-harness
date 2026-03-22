//! # aura-kernel
//!
//! Deterministic kernel and turn processor for Aura.
//!
//! This crate provides:
//! - Single-step kernel processing (Spec-01 legacy)
//! - Multi-step turn processor for agentic loops (Spec-02)
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
//! ## Turn Processor (Spec-02)
//!
//! The `TurnProcessor` implements a Claude Code-like agentic loop:
//!
//! ```text
//! loop {
//!     1. Build context (deterministic)
//!     2. Call ModelProvider.complete()
//!     3. Record assistant response
//!     4. If tool_use: authorize → execute → inject tool_result
//!     5. If end_turn: finalize
//! }
//! ```

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic, clippy::nursery)]

mod context;
mod kernel;
mod policy;
pub mod process_manager;
mod turn_processor;

pub use context::{Context, ContextBuilder};
pub use kernel::{Kernel, KernelConfig, ProcessResult};
pub use policy::{default_tool_permission, PermissionLevel, Policy, PolicyConfig, PolicyResult};
pub use process_manager::{ProcessManager, ProcessManagerConfig, ProcessOutput, RunningProcess};
pub use turn_processor::{
    ExecutedToolCall, ModelCallDelegate, StepConfig, StepResult, StreamCallback,
    StreamCallbackEvent, ToolCache, TurnConfig, TurnEntry, TurnProcessor, TurnResult,
};

// Re-export ToolResultContent for convenience
pub use aura_reasoner::ToolResultContent;
