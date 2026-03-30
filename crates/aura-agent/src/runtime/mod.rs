//! # aura-runtime
//!
//! Turn processor and process manager for Aura.
//!
//! This module provides:
//! - Multi-step turn processor for agentic loops (Spec-02)
//! - Process manager for async command execution

pub mod process_manager;
mod turn_processor;

pub use process_manager::{ProcessManager, ProcessManagerConfig, ProcessOutput, RunningProcess};
pub use turn_processor::{
    ExecutedToolCall, ModelCallDelegate, StepConfig, StepResult, StreamCallback,
    StreamCallbackEvent, ToolCache, TurnConfig, TurnEntry, TurnProcessor, TurnResult,
};

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("model error: {0}")]
    Model(String),
    #[error("tool execution error: {0}")]
    ToolExecution(String),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("store error: {0}")]
    Store(String),
    #[error("{0}")]
    Internal(String),
}

impl From<aura_reasoner::ReasonerError> for RuntimeError {
    fn from(e: aura_reasoner::ReasonerError) -> Self {
        RuntimeError::Model(e.to_string())
    }
}
