//! The `run_blocks` tool the coordinator calls.

use std::sync::Arc;

use async_trait::async_trait;
use aura_core_types::{Capability, ToolDefinition, ToolResult};
use aura_tools::{Tool, ToolContext, ToolError};

use crate::block::RunBlocksInput;
use crate::runner::BlockRunner;

/// Tool name.
pub const RUN_BLOCKS_TOOL: &str = "run_blocks";

/// Hands blocks to a [`BlockRunner`].
pub struct RunBlocksTool {
    runner: Arc<BlockRunner>,
}

impl RunBlocksTool {
    /// Tool backed by `runner`.
    #[must_use]
    pub const fn new(runner: Arc<BlockRunner>) -> Self {
        Self { runner }
    }

    /// Schema shown to the coordinator.
    #[must_use]
    pub fn definition() -> ToolDefinition {
        let commands = serde_json::json!({ "type": "array", "items": { "type": "string" } });
        ToolDefinition::new(
            RUN_BLOCKS_TOOL,
            "Hand blocks of work to cheaper worker models, in order. Each block goes to the \
             cheapest model tier likely to pass its checks and is retried on stronger tiers \
             with the failure evidence. Returns per-block status, verification level, files \
             changed, the workers' summaries and cost. Blocks after a failed one are skipped.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "blocks": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": { "type": "string", "description": "Short unique name." },
                                "kind": {
                                    "type": "string",
                                    "enum": ["command", "explore", "edit", "test", "refactor", "docs"],
                                    "description": "command runs `command` with no model; explore reads and reports; the rest change files."
                                },
                                "instructions": { "type": "string", "description": "Self-contained description of the work. Workers see nothing else of this conversation." },
                                "command": { "type": "string", "description": "Shell command for kind=command." },
                                "files": { "type": "array", "items": { "type": "string" }, "description": "Files to start from." },
                                "context": { "type": "string", "description": "Decisions, signatures and excerpts the worker needs." },
                                "build": { "type": "string", "description": "Build command that must pass." },
                                "checks": commands.clone(),
                                "acceptance": commands,
                                "criteria": { "type": "array", "items": { "type": "string" }, "description": "What done looks like." },
                                "review": { "type": "boolean", "description": "Also require an independent reviewer's approval." },
                                "risk": { "type": "string", "enum": ["low", "medium", "high"] },
                                "min_tier": { "type": "string", "enum": ["fast", "standard", "frontier"] }
                            },
                            "required": ["title", "kind"]
                        }
                    }
                },
                "required": ["blocks"]
            }),
        )
    }
}

#[async_trait]
impl Tool for RunBlocksTool {
    fn name(&self) -> &str {
        RUN_BLOCKS_TOOL
    }

    fn definition(&self) -> ToolDefinition {
        Self::definition()
    }

    fn required_capabilities(&self) -> Vec<Capability> {
        vec![Capability::SpawnAgent]
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let input: RunBlocksInput = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("run_blocks: {e}")))?;
        let output = match self.runner.run(ctx, &input.blocks).await {
            Ok(output) => output,
            Err(e) => return Ok(ToolResult::failure(RUN_BLOCKS_TOOL, e)),
        };
        let body = serde_json::to_vec(&output)
            .map_err(|e| ToolError::Serialization(format!("run_blocks: {e}")))?;
        let result = if output.failed == 0 {
            ToolResult::success(RUN_BLOCKS_TOOL, body)
        } else {
            ToolResult::failure(RUN_BLOCKS_TOOL, body)
        };
        Ok(result.with_metadata("gate_cost_usd", format!("{:.6}", output.cost_usd)))
    }
}
