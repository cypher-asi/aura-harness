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
pub mod blocking;
mod budget;
pub mod policy;
pub mod build;
pub mod compaction;
mod constants;
pub mod file_ops;
pub mod planning;
pub mod prompts;
pub mod events;
mod helpers;
mod kernel_executor;
mod read_guard;
mod sanitize;
pub mod git;
pub mod parser;
pub mod self_review;
pub mod shell_parse;
pub mod types;
pub mod verify;

pub mod agent_runner;
pub mod task_context;
pub mod task_executor;
pub mod message_conversion;

pub use agent_loop::{AgentLoop, AgentLoopConfig};
pub use aura_kernel::ModelCallDelegate;
pub use events::AgentLoopEvent;
pub use kernel_executor::KernelToolExecutor;
pub use types::{
    AgentLoopResult, AgentToolExecutor, AutoBuildResult, BuildBaseline, ToolCallInfo,
    ToolCallResult,
};

#[cfg(test)]
mod event_sequence_tests;
