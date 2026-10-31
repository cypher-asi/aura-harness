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
use aura_core_types::{ToolProposal, Transaction};
use aura_store_db::DequeueToken;
use tracing::instrument;

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
        batch::process_many(self, tool_proposals).await
    }
}
