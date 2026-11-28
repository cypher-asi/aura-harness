//! The Runpod tools: one [`RunpodTool`] per [`RunpodOp`], all sharing a
//! [`RunpodClient`].

mod args;
mod read;
mod schemas;
mod spend;

use super::output::render;
use super::{RunpodClient, RunpodError};
use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core_types::{ToolDefinition, ToolResult};
use serde_json::Value;

/// What a tool's calls do, for the approval prompt and the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    /// Reads account, catalog, pod or job state.
    Read,
    /// Can start paid compute.
    Spends,
    /// Destroys a resource and its data.
    Destroys,
}

/// One Runpod operation an agent can call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunpodOp {
    ListGpuTypes,
    ListPods,
    GetPod,
    PodLogs,
    Balance,
    JobStatus,
    CreatePod,
    PodAction,
    RunJob,
    TerminatePod,
}

impl RunpodOp {
    /// Every operation, reads first.
    pub const ALL: [Self; 10] = [
        Self::ListGpuTypes,
        Self::ListPods,
        Self::GetPod,
        Self::PodLogs,
        Self::Balance,
        Self::JobStatus,
        Self::CreatePod,
        Self::PodAction,
        Self::RunJob,
        Self::TerminatePod,
    ];

    /// The tool name the model calls.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::ListGpuTypes => "runpod_list_gpu_types",
            Self::ListPods => "runpod_list_pods",
            Self::GetPod => "runpod_get_pod",
            Self::PodLogs => "runpod_pod_logs",
            Self::Balance => "runpod_balance",
            Self::JobStatus => "runpod_job_status",
            Self::CreatePod => "runpod_create_pod",
            Self::PodAction => "runpod_pod_action",
            Self::RunJob => "runpod_run_job",
            Self::TerminatePod => "runpod_terminate_pod",
        }
    }

    /// Whether calls read, spend or destroy.
    #[must_use]
    pub const fn kind(self) -> ToolKind {
        match self {
            Self::ListGpuTypes
            | Self::ListPods
            | Self::GetPod
            | Self::PodLogs
            | Self::Balance
            | Self::JobStatus => ToolKind::Read,
            Self::CreatePod | Self::PodAction | Self::RunJob => ToolKind::Spends,
            Self::TerminatePod => ToolKind::Destroys,
        }
    }

    /// The operation named `name`, if it is a Runpod tool.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|op| op.name() == name)
    }
}

/// A Runpod tool, ready to register on an executor.
#[derive(Debug, Clone)]
pub struct RunpodTool {
    op: RunpodOp,
    client: RunpodClient,
}

impl RunpodTool {
    /// The tool for `op`, sending through `client`.
    #[must_use]
    pub const fn new(op: RunpodOp, client: RunpodClient) -> Self {
        Self { op, client }
    }

    /// Its operation.
    #[must_use]
    pub const fn op(&self) -> RunpodOp {
        self.op
    }

    async fn run(&self, args: &Value) -> Result<Value, RunpodError> {
        let client = &self.client;
        match self.op {
            RunpodOp::ListGpuTypes => read::list_gpu_types(client, args).await,
            RunpodOp::ListPods => read::list_pods(client, args).await,
            RunpodOp::GetPod => read::get_pod(client, args).await,
            RunpodOp::PodLogs => read::pod_logs(client, args).await,
            RunpodOp::Balance => read::balance(client).await,
            RunpodOp::JobStatus => read::job_status(client, args).await,
            RunpodOp::CreatePod => spend::create_pod(client, args).await,
            RunpodOp::PodAction => spend::pod_action(client, args).await,
            RunpodOp::RunJob => spend::run_job(client, args).await,
            RunpodOp::TerminatePod => spend::terminate_pod(client, args).await,
        }
    }
}

/// Every Runpod tool, sharing `client`.
#[must_use]
pub fn runpod_tools(client: &RunpodClient) -> Vec<RunpodTool> {
    RunpodOp::ALL
        .into_iter()
        .map(|op| RunpodTool::new(op, client.clone()))
        .collect()
}

#[async_trait]
impl Tool for RunpodTool {
    fn name(&self) -> &str {
        self.op.name()
    }

    fn definition(&self) -> ToolDefinition {
        schemas::definition(self.op, self.client.settings())
    }

    async fn execute(&self, _ctx: &ToolContext, args: Value) -> Result<ToolResult, ToolError> {
        let name = self.op.name();
        Ok(match self.run(&args).await {
            Ok(value) => ToolResult::success(name, render(&value)),
            Err(e) => ToolResult::failure(name, e.to_string()),
        })
    }
}
