//! Layer: agent
//!
//! The cost gate.
//!
//! A gate turn runs on a frontier model acting as coordinator. It plans
//! and hands blocks of work to `run_blocks`; the gate starts each block
//! on the cheapest model tier likely to pass its checks, verifies the
//! result (build, visible checks, hidden acceptance checks, optional
//! review), and on failure retries once in place, then escalates a tier
//! from the pre-block files with the failure evidence. Failed blocks go
//! back to the coordinator to re-plan.
//!
//! - [`tier`]: tiers and `gate/<frontier>` model ids
//! - [`block`]: block specs and results
//! - [`policy`]: rule table, earned tiers and expected-cost choice
//! - [`ladder`]: retry and escalation
//! - [`verify`]: running checks through `run_command`
//! - [`undo`]: pre-block snapshots
//! - [`runner`]: [`BlockRunner`], which ties them together
//! - [`tool`]: the `run_blocks` tool
//! - [`ledger`] and [`fit`]: the attempt log and policy fitting
//!
//! The gate never calls a model itself: workers are child agents started
//! through the caller's subagent dispatcher, so their model calls go
//! through their own kernels.

pub mod block;
pub mod fit;
pub mod ladder;
pub mod ledger;
pub mod policy;
pub mod prompts;
pub mod runner;
pub mod tier;
pub mod tool;
pub mod undo;
pub mod verify;

pub use block::{BlockKind, BlockResult, BlockSpec, BlockStatus, RunBlocksOutput, VerifyLevel};
pub use ledger::{Ledger, LedgerEntry, LEDGER_FILE};
pub use policy::{GatePolicy, RULES_VERSION};
pub use runner::{BlockRunner, GateSettings, GATE_BLOCK_AGENT, READ_TOOLS, WRITE_TOOLS};
pub use tier::{frontier_of, gate_id, Tier, Tiers, GATE_PREFIX};
pub use tool::{RunBlocksTool, RUN_BLOCKS_TOOL};

use aura_core_types::ToolDefinition;

/// The coordinator's tools: `tools` plus `run_blocks`, without the file
/// editing tools unless `coordinator_edits`.
#[must_use]
pub fn coordinator_tools(
    tools: Vec<ToolDefinition>,
    coordinator_edits: bool,
) -> Vec<ToolDefinition> {
    let mut out: Vec<ToolDefinition> = tools
        .into_iter()
        .filter(|t| coordinator_edits || !WRITE_TOOLS.contains(&t.name.as_str()))
        .filter(|t| t.name != RUN_BLOCKS_TOOL)
        .collect();
    out.push(RunBlocksTool::definition());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinator_loses_edit_tools_and_gains_run_blocks() {
        let tools = ["read_file", "edit_file", "write_file", "run_command"]
            .into_iter()
            .map(|n| ToolDefinition::new(n, "", serde_json::json!({})))
            .collect::<Vec<_>>();
        let names = |t: &[ToolDefinition]| t.iter().map(|d| d.name.clone()).collect::<Vec<_>>();
        assert_eq!(
            names(&coordinator_tools(tools.clone(), false)),
            ["read_file", "run_command", "run_blocks"]
        );
        assert_eq!(coordinator_tools(tools, true).len(), 5);
    }
}
