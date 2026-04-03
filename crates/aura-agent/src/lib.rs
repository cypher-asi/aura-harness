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
// Line/column numbers and small counters never exceed i32::MAX or lose f64 precision
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_precision_loss)]
// Internal crate — error docs for pub(crate) functions add noise
#![allow(clippy::missing_errors_doc)]
// Prompt-building code uses push_str(&format!()) extensively for clarity
#![allow(clippy::format_push_string)]
// Many match-to-let-else refactors would reduce readability in complex control flow
#![allow(clippy::manual_let_else)]
// Mutex guard drop timing is correct; tightening adds complexity for marginal benefit
#![allow(clippy::significant_drop_tightening)]
// Result wrappers kept for forward-compatibility (functions may gain error paths)
#![allow(clippy::unnecessary_wraps)]
// if-let-else is often more readable than map_or/map_or_else closures
#![allow(clippy::option_if_let_else)]

mod agent_loop;
pub(crate) mod blocking;
mod budget;
pub(crate) mod build;
pub(crate) mod compaction;
pub mod constants;
pub(crate) mod events;
// TODO: file_ops submodules are WIP — remove allow once integrated
#[allow(dead_code)]
pub(crate) mod file_ops;
pub mod git;
// TODO: helpers has unused utility functions — remove allow once integrated
#[allow(dead_code)]
mod helpers;
mod kernel_executor;
mod kernel_gateway;
// TODO: parser is WIP — remove allow once integrated
#[allow(dead_code)]
pub(crate) mod parser;
// TODO: planning is WIP — remove allow once integrated
#[allow(dead_code)]
pub(crate) mod planning;
pub(crate) mod policy;
pub mod prompts;
mod read_guard;
mod sanitize;
pub(crate) mod self_review;
// TODO: shell_parse is WIP — remove allow once integrated
#[allow(dead_code)]
pub(crate) mod shell_parse;
pub mod types;
// TODO: verify module is WIP — remove allow once integrated
#[allow(dead_code)]
pub(crate) mod verify;

pub mod agent_runner;
// TODO: message_conversion is WIP — remove allow once integrated
#[allow(dead_code)]
pub(crate) mod message_conversion;
pub mod runtime;
pub mod session_bootstrap;
// TODO: task_context is WIP — remove allow once integrated
#[allow(dead_code)]
pub(crate) mod task_context;
// TODO: task_executor is WIP — remove allow once integrated
#[allow(dead_code)]
pub(crate) mod task_executor;

pub use agent_loop::{AgentLoop, AgentLoopConfig};
pub use constants::{tool_result_cache_key, CACHEABLE_TOOLS, DEFAULT_MODEL, FALLBACK_MODEL};
pub use events::{AgentLoopEvent, TurnEvent};
#[allow(deprecated)]
pub use kernel_executor::KernelToolExecutor;
pub use kernel_gateway::{KernelModelGateway, KernelToolGateway};
pub use runtime::{
    ProcessManager, ProcessManagerConfig, ProcessOutput, RunningProcess, RuntimeError,
};
pub use types::{
    AgentLoopResult, AgentToolExecutor, AutoBuildResult, BuildBaseline, ToolCallInfo,
    ToolCallResult, TurnObserver, TurnObservers,
};

/// Errors arising from the agent orchestration loop (model calls, tool execution, timeouts).
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
        match e {
            aura_reasoner::ReasonerError::Timeout => {
                Self::Timeout("model request timed out".to_string())
            }
            aura_reasoner::ReasonerError::InsufficientCredits(msg) => {
                Self::Model(format!("insufficient credits: {msg}"))
            }
            aura_reasoner::ReasonerError::RateLimited(msg) => {
                Self::Model(format!("rate limited: {msg}"))
            }
            aura_reasoner::ReasonerError::Api { status, message } => {
                Self::Model(format!("api error ({status}): {message}"))
            }
            aura_reasoner::ReasonerError::Request(msg) => {
                Self::Model(format!("request error: {msg}"))
            }
            aura_reasoner::ReasonerError::Parse(msg) => Self::Model(format!("parse error: {msg}")),
            aura_reasoner::ReasonerError::Internal(msg) => Self::Model(msg),
        }
    }
}

#[cfg(test)]
mod event_sequence_tests;
#[cfg(test)]
mod store_migration_tests;
