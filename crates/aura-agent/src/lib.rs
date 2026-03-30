//! # aura-agent
//!
//! Multi-step agentic orchestration layer for AURA.
//!
//! This crate owns the intelligent agent loop that wraps the kernel's
//! single-step processing. It provides:
//!
//! - `AgentLoop` — the main multi-step orchestrator
//! - Blocking detection — prevents infinite loops on failing tools
//! - Read guards — limits redundant file re-reads
//! - Context compaction — tiered message truncation to stay within token limits
//! - Message sanitization — repairs message history for API validity
//! - Budget tracking — exploration, token, and credit budget management
//! - Build integration — auto-build checks after write operations
//!
//! ## Architecture
//!
//! `aura-agent` sits between the presentation layer (CLI, terminal, swarm)
//! and the kernel. It calls the step processor in a loop, adding intelligence
//! at each iteration.
//!
//! ```text
//! Presentation → AgentLoop → StepProcessor → ModelProvider + Tools
//! ```

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]
// Phase 1: most code is staged for wiring in Phase 4.
#![allow(dead_code)]

mod agent_loop;
pub(crate) mod blocking;
mod budget;
pub(crate) mod build;
pub(crate) mod compaction;
mod constants;
pub mod events;
pub(crate) mod file_ops;
pub mod git;
mod helpers;
mod kernel_executor;
pub(crate) mod parser;
pub(crate) mod planning;
pub(crate) mod policy;
pub mod prompts;
mod read_guard;
mod sanitize;
pub(crate) mod self_review;
pub(crate) mod shell_parse;
pub mod types;
pub(crate) mod verify;

pub mod agent_runner;
pub(crate) mod message_conversion;
pub mod runtime;
pub(crate) mod task_context;
pub(crate) mod task_executor;

pub use agent_loop::{AgentLoop, AgentLoopConfig};
pub use events::AgentLoopEvent;
pub use kernel_executor::KernelToolExecutor;
pub use runtime::{
    ExecutedToolCall, ModelCallDelegate, ProcessManager, ProcessManagerConfig, ProcessOutput,
    RunningProcess, RuntimeError, StepConfig, StepResult, StreamCallback, StreamCallbackEvent,
    ToolCache, TurnConfig, TurnEntry, TurnProcessor, TurnResult,
};
pub use types::{
    AgentLoopResult, AgentToolExecutor, AutoBuildResult, BuildBaseline, ToolCallInfo,
    ToolCallResult,
};

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("model error: {0}")]
    Model(String),
    #[error("tool execution error: {0}")]
    ToolExecution(String),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("build failed: {0}")]
    BuildFailed(String),
    #[error("{0}")]
    Internal(String),
}

impl From<aura_reasoner::ReasonerError> for AgentError {
    fn from(e: aura_reasoner::ReasonerError) -> Self {
        AgentError::Model(e.to_string())
    }
}

#[cfg(test)]
mod event_sequence_tests;
