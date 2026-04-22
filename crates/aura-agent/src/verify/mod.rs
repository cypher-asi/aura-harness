//! Build and test verification with automatic fix loops.
//!
//! This module provides standalone functions for:
//! - Running build/test commands and capturing results ([`runner`])
//! - Normalizing compiler error signatures for stagnation detection ([`signatures`])
//! - Extracting type/method references from compiler errors ([`error_types`])
//! - File snapshot/rollback and build command inference ([`utils`])
//! - Full build-verify-fix orchestration ([`build`], [`test`])
//!
//! ## Usage
//!
//! Callers implement [`FixProvider`] to supply LLM fix generation and response
//! parsing, then call [`build::verify_and_fix_build`] to run the loop. Events
//! are streamed through an optional `UnboundedSender<VerifyEvent>`.
//
// TODO(phase5, 2026-04-22): the `verify_and_fix_build` loop plus its
// helpers (snapshot/rollback, test runners, error stagnation) are not
// wired into the agent loop yet — only error-context helpers and
// run_build_command are actively invoked. Phase 5 is expected to
// integrate the full fix-loop. Until then, silence dead-code at the
// module root rather than per-function.
#![allow(dead_code)]

use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;

use crate::file_ops::FileOp;

pub mod build;
pub mod common;
pub mod error_types;
pub mod runner;
pub mod signatures;
pub mod test;
pub mod utils;

pub use error_types::{parse_error_references, BuildFixAttemptRecord};
pub use runner::run_build_command;
pub use signatures::normalize_error_signature;
pub use utils::{auto_correct_build_command, infer_default_build_command};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the verify-and-fix loop.
pub struct VerifyConfig {
    /// Maximum number of build-fix iterations before giving up.
    pub max_build_fix_retries: u32,
}

impl Default for VerifyConfig {
    fn default() -> Self {
        Self {
            max_build_fix_retries: 4,
        }
    }
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Events emitted during verification for observability and forwarding.
#[derive(Debug, Clone)]
pub enum VerifyEvent {
    BuildStarted {
        command: String,
    },
    BuildPassed {
        command: String,
        stdout: String,
        duration_ms: u64,
    },
    BuildFailed {
        command: String,
        stdout: String,
        stderr: String,
        attempt: u32,
        duration_ms: u64,
    },
    BuildSkipped {
        reason: String,
    },
    BuildFixAttempt {
        attempt: u32,
    },
    TestStarted {
        command: String,
    },
    TestPassed {
        command: String,
        stdout: String,
        summary: String,
        duration_ms: u64,
    },
    TestFailed {
        command: String,
        stdout: String,
        stderr: String,
        attempt: u32,
        summary: String,
        duration_ms: u64,
    },
    TestFixAttempt {
        attempt: u32,
    },
    /// Incremental stdout/stderr line from a running build command.
    OutputDelta(String),
}

// ---------------------------------------------------------------------------
// Fix provider trait
// ---------------------------------------------------------------------------

/// Callback trait for requesting LLM-generated fixes and parsing responses.
///
/// Implementors supply the model interaction and response parsing that is
/// specific to their orchestration layer (prompt construction, model choice,
/// billing, etc.).
#[async_trait]
pub trait FixProvider: Send + Sync {
    /// Request a code fix from the LLM given build/test error output.
    ///
    /// Returns `(response_text, input_tokens, output_tokens)`.
    async fn request_fix(
        &self,
        command: &str,
        stderr: &str,
        stdout: &str,
        prior_attempts: &[BuildFixAttemptRecord],
    ) -> anyhow::Result<(String, u64, u64)>;

    /// Parse an LLM fix response into file operations.
    fn parse_fix_response(&self, response: &str) -> anyhow::Result<Vec<FileOp>>;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn emit(tx: Option<&UnboundedSender<VerifyEvent>>, event: VerifyEvent) {
    if let Some(tx) = tx {
        let _ = tx.send(event);
    }
}
