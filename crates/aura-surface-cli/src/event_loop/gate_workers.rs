//! Gate blocks in the terminal: a coordinator's `run_blocks` call runs
//! each block as a short-lived worker agent in the coordinator's folder
//! and mode. Workers have no chat; their approval prompts appear in the
//! coordinator's, and the Processes tab lists them under it.

use std::sync::Arc;

use aura_agent::AgentLoopConfig;
use aura_agent_gate::{BlockRunner, RunBlocksTool};
use aura_core_types::{AgentId, SubagentDispatchRequest, SubagentExit, SubagentResult};
use aura_model_reasoner::ToolDefinition;
use aura_terminal::events::{ProcessKind, ProcessStatus, ProcessSummary};
use tokio_util::sync::CancellationToken;

use super::approvals::TerminalApprovals;
use super::processes::{now_ms, task_id};
use super::subagents::{lock, AgentHub};

/// What the hub needs to run gate coordinators and their workers.
pub struct GateHost {
    pub runner: Arc<BlockRunner>,
    pub approvals: Option<Arc<TerminalApprovals>>,
}

impl AgentHub {
    /// When `config` selects a gate model, make `agent`'s turn a
    /// coordinator turn.
    pub(super) fn coordinate(
        &self,
        agent: AgentId,
        config: &mut AgentLoopConfig,
        mut tools: Vec<ToolDefinition>,
    ) -> Vec<ToolDefinition> {
        let Some(gate) = &self.gate else {
            return tools;
        };
        if crate::gate::frontier_of(&config.model).is_none() {
            return tools;
        }
        tools.push(RunBlocksTool::definition());
        let limits = lock(&self.base).limits;
        crate::gate::coordinate(&gate.runner, agent, config, tools, Some(limits))
    }

    /// The runner to draft a turn with, when `tools` are a coordinator's.
    pub(crate) fn gate_drafter(&self, tools: &[ToolDefinition]) -> Option<Arc<BlockRunner>> {
        let gate = self.gate.as_ref()?;
        tools
            .iter()
            .any(|t| t.name == crate::gate::RUN_BLOCKS_TOOL)
            .then(|| Arc::clone(&gate.runner))
    }

    /// Run one block of a coordinator's `run_blocks` call.
    pub(super) async fn run_gate_block(
        &self,
        request: SubagentDispatchRequest,
    ) -> Result<SubagentResult, String> {
        let Some(gate) = &self.gate else {
            return Ok(SubagentResult::rejected("run_blocks: the gate is off"));
        };
        let parent = request.parent_agent_id;
        let folder = match self.running_kernel(parent) {
            Some(kernel) => Some(kernel.agent_workspace()),
            None => lock(&self.registry).project_folder(parent),
        };
        let child = AgentId::generate();
        let kernel = self
            .factory
            .build(child, request.parent_chain.clone(), folder)
            .map_err(|e| format!("starting a gate worker: {e:#}"))?;
        let kernel = Arc::new(kernel);
        let workspace = kernel.agent_workspace();
        super::modes::apply(&kernel, self.mode_of(parent), &self.tool_names());
        if let Some(approvals) = &gate.approvals {
            approvals.show_in(child, parent);
        }
        let cancel = self
            .running_cancel(parent)
            .map_or_else(CancellationToken::new, |token| token.child_token());
        let (config, tools) = self.turn(None, &workspace);
        self.processes
            .start(worker_summary(&request, child), Some(cancel.clone()));

        let result = crate::gate::run_worker(kernel, config, tools, &request, cancel).await;

        self.processes.finish(
            &task_id(child),
            if result.exit == SubagentExit::Completed {
                ProcessStatus::Exited
            } else {
                ProcessStatus::Failed
            },
        );
        if let Some(approvals) = &gate.approvals {
            approvals.stop_showing_in(child);
        }
        Ok(result)
    }
}

/// Processes tab row for a gate worker; jumping to it opens its
/// coordinator.
fn worker_summary(request: &SubagentDispatchRequest, child: AgentId) -> ProcessSummary {
    let owner = request.parent_agent_id;
    let model = request.model_override.clone().unwrap_or_default();
    let title = request
        .prompt
        .lines()
        .find_map(|line| line.strip_prefix("# "))
        .unwrap_or("block")
        .to_string();
    ProcessSummary {
        id: task_id(child),
        kind: ProcessKind::Task,
        label: format!("gate: {title} ({model})"),
        detail: request.prompt.clone(),
        status: ProcessStatus::Running,
        owner_agent_id: owner.to_hex(),
        owner_name: String::new(),
        parent_chain: request
            .parent_chain
            .iter()
            .filter(|id| **id != owner)
            .map(AgentId::to_hex)
            .collect(),
        started_ms: now_ms(),
        tool_use_id: request.tool_call_id.clone(),
        model: Some(model),
        jump_agent_id: owner.to_hex(),
        killable: true,
        ..ProcessSummary::default()
    }
}
