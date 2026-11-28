//! Async process integration tests.
//!
//! Tests the full async process flow including pending effects and completion transactions.

// Integration-test crate: test code may unwrap/expect.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use aura_agent::ProcessManager;
use aura_core_types::{
    ActionId, ActionResultPayload, AgentId, Hash, ProcessId, ProcessPending, Transaction,
    TransactionType,
};
use aura_exec_tools::{cmd_run_with_threshold, Sandbox, ThresholdResult};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// Create a test workspace.
fn create_test_workspace() -> TempDir {
    TempDir::new().unwrap()
}

mod chain_integrity;
mod concurrent;
mod payloads;
mod sync_vs_async;
