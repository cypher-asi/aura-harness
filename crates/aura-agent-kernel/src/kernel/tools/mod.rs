//! Tool-proposal processing.
//!
//! Single-proposal ([`Kernel::process_tool_proposal`]) and batch
//! ([`Kernel::process_tools`]) paths both record every tool call as
//! three linked entries (Invariant §5):
//! 1. `ToolProposal` — appended before the policy runs.
//! 2. `PolicyDecision` — the verdict of the full
//!    `Policy::evaluate_proposal` pipeline (Invariant §4) plus any live
//!    `ask` resolution, naming the rule that decided. Appended before
//!    execution and referencing the proposal.
//! 3. `ToolExecution` — approved tools only, executed via
//!    `ExecutorRouter`; carries the action and effect and references the
//!    decision.
//!
//! The implementation is split for readability:
//! - [`shared`] — helpers used by both paths (`execute_with_timeout`,
//!   `kernel_proposal_from_tool_proposal`, `resolve_live_ask`).
//! - [`steps`] — appending the three entries and building the
//!   `ProcessResult`.
//! - [`single`] — the single-proposal flow.
//! - [`batch`] — the batch flow.

mod batch;
mod shared;
mod single;
mod steps;

use super::{Kernel, ProcessResult};
use aura_core_types::{ToolProposal, ToolResultKind, Transaction};
use aura_store_db::DequeueToken;
use std::future::Future;
use std::pin::Pin;
use tracing::instrument;

/// A tool result produced outside the executor router: one the caller
/// already has, or one its own handler returned.
#[derive(Debug, Clone)]
pub struct PresetToolResult {
    pub content: String,
    pub is_error: bool,
    pub kind: ToolResultKind,
}

/// How an allowed tool call gets its result. Every variant is recorded
/// and gated the same way; only the step after an allow differs.
pub enum ToolRun<'a> {
    /// Dispatch through the executor router.
    Executor,
    /// Use a result the caller already has (a cached read, a steering
    /// block, an invalid call).
    Preset(PresetToolResult),
    /// Run a handler the caller owns (an engine tool such as
    /// `task_done`). The future is polled only when policy allows the
    /// call, and dropped unpolled otherwise, so it must not start work
    /// before its first poll.
    Local(Pin<Box<dyn Future<Output = PresetToolResult> + Send + 'a>>),
}

impl std::fmt::Debug for ToolRun<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Executor => f.write_str("Executor"),
            Self::Preset(preset) => f.debug_tuple("Preset").field(preset).finish(),
            Self::Local(_) => f.write_str("Local(..)"),
        }
    }
}

impl Kernel {
    #[instrument(skip(self, tx, token))]
    pub(super) async fn process_tool_proposal(
        &self,
        tx: Transaction,
        token: Option<DequeueToken>,
    ) -> Result<ProcessResult, crate::KernelError> {
        single::process_one(self, tx, token).await
    }

    /// Process a batch of tool proposals, executing approved tools in parallel.
    ///
    /// # Errors
    /// Returns error if serialization, execution, or storage fails.
    pub async fn process_tools(
        &self,
        tool_proposals: Vec<ToolProposal>,
    ) -> Result<Vec<ProcessResult>, crate::KernelError> {
        let calls = tool_proposals
            .into_iter()
            .map(|p| (p, ToolRun::Executor))
            .collect();
        batch::process_many(self, calls).await
    }

    /// Process a batch of tool calls, each saying how it runs once policy
    /// allows it (see [`ToolRun`]).
    ///
    /// # Errors
    /// Returns error if serialization, execution, or storage fails.
    pub async fn process_tool_runs(
        &self,
        calls: Vec<(ToolProposal, ToolRun<'_>)>,
    ) -> Result<Vec<ProcessResult>, crate::KernelError> {
        batch::process_many(self, calls).await
    }
}
