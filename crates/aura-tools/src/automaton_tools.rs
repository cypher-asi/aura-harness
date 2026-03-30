//! Dev-loop control tools (`start_dev_loop`, `pause_dev_loop`, `stop_dev_loop`,
//! `run_task`).
//!
//! These tools let the chat agent manage automaton lifecycle within the harness.
//! Actual automaton operations are delegated to an [`AutomatonController`] trait
//! whose concrete implementation lives in the node layer (avoiding circular deps
//! with `aura-automaton`).

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use aura_core::ToolDefinition;
use aura_core::ToolResult;

use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};

// ---------------------------------------------------------------------------
// Controller trait (implemented in aura-node)
// ---------------------------------------------------------------------------

/// Abstraction over automaton lifecycle so tools don't depend on `aura-automaton`.
#[async_trait]
pub trait AutomatonController: Send + Sync {
    /// Install and start a dev-loop automaton for `project_id`.
    /// Returns the automaton ID on success.
    async fn start_dev_loop(
        &self,
        project_id: &str,
        workspace_root: Option<PathBuf>,
        auth_token: Option<String>,
        model: Option<String>,
        git_repo_url: Option<String>,
        git_branch: Option<String>,
    ) -> Result<String, String>;

    /// Pause the running dev-loop for `project_id`.
    async fn pause_dev_loop(&self, project_id: &str) -> Result<(), String>;

    /// Stop (cancel) the running dev-loop for `project_id`.
    async fn stop_dev_loop(&self, project_id: &str) -> Result<(), String>;

    /// Execute a single task through the dev-loop engine (non-blocking).
    /// Returns the automaton ID immediately.
    #[allow(clippy::too_many_arguments)]
    async fn run_task(
        &self,
        project_id: &str,
        task_id: &str,
        workspace_root: Option<PathBuf>,
        auth_token: Option<String>,
        model: Option<String>,
        git_repo_url: Option<String>,
        git_branch: Option<String>,
    ) -> Result<String, String>;
}

// ---------------------------------------------------------------------------
// start_dev_loop
// ---------------------------------------------------------------------------

pub struct StartDevLoopTool {
    controller: Arc<dyn AutomatonController>,
    project_id: String,
    workspace_root: Option<PathBuf>,
    auth_token: Option<String>,
}

impl StartDevLoopTool {
    pub fn new(
        controller: Arc<dyn AutomatonController>,
        project_id: String,
        workspace_root: Option<PathBuf>,
        auth_token: Option<String>,
    ) -> Self {
        Self {
            controller,
            project_id,
            workspace_root,
            auth_token,
        }
    }
}

#[async_trait]
impl Tool for StartDevLoopTool {
    fn name(&self) -> &str {
        "start_dev_loop"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "start_dev_loop".into(),
            description: "Start the autonomous dev loop for the project. It will pick up ready tasks and execute them.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "model": { "type": "string", "description": "Optional model override for the loop (e.g. 'claude-sonnet-4-20250514')" }
                },
                "required": []
            }),
            cache_control: None,
        }
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let model = args.get("model").and_then(|v| v.as_str()).map(String::from);

        match self
            .controller
            .start_dev_loop(
                &self.project_id,
                self.workspace_root.clone(),
                self.auth_token.clone(),
                model,
                None,
                None,
            )
            .await
        {
            Ok(automaton_id) => Ok(ToolResult::success(
                "start_dev_loop",
                format!("Dev loop started (automaton_id: {automaton_id}). Monitor progress via /stream/automaton/{automaton_id}"),
            )),
            Err(e) => Ok(ToolResult::failure("start_dev_loop", e)),
        }
    }
}

// ---------------------------------------------------------------------------
// pause_dev_loop
// ---------------------------------------------------------------------------

pub struct PauseDevLoopTool {
    controller: Arc<dyn AutomatonController>,
    project_id: String,
}

impl PauseDevLoopTool {
    pub fn new(controller: Arc<dyn AutomatonController>, project_id: String) -> Self {
        Self {
            controller,
            project_id,
        }
    }
}

#[async_trait]
impl Tool for PauseDevLoopTool {
    fn name(&self) -> &str {
        "pause_dev_loop"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "pause_dev_loop".into(),
            description: "Pause the currently running dev loop.".into(),
            input_schema: serde_json::json!({"type":"object","properties":{},"required":[]}),
            cache_control: None,
        }
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        _args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        match self.controller.pause_dev_loop(&self.project_id).await {
            Ok(()) => Ok(ToolResult::success("pause_dev_loop", "Dev loop paused")),
            Err(e) => Ok(ToolResult::failure("pause_dev_loop", e)),
        }
    }
}

// ---------------------------------------------------------------------------
// stop_dev_loop
// ---------------------------------------------------------------------------

pub struct StopDevLoopTool {
    controller: Arc<dyn AutomatonController>,
    project_id: String,
}

impl StopDevLoopTool {
    pub fn new(controller: Arc<dyn AutomatonController>, project_id: String) -> Self {
        Self {
            controller,
            project_id,
        }
    }
}

#[async_trait]
impl Tool for StopDevLoopTool {
    fn name(&self) -> &str {
        "stop_dev_loop"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "stop_dev_loop".into(),
            description: "Stop the currently running dev loop.".into(),
            input_schema: serde_json::json!({"type":"object","properties":{},"required":[]}),
            cache_control: None,
        }
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        _args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        match self.controller.stop_dev_loop(&self.project_id).await {
            Ok(()) => Ok(ToolResult::success("stop_dev_loop", "Dev loop stopped")),
            Err(e) => Ok(ToolResult::failure("stop_dev_loop", e)),
        }
    }
}

// ---------------------------------------------------------------------------
// run_task
// ---------------------------------------------------------------------------

pub struct RunTaskTool {
    controller: Arc<dyn AutomatonController>,
    project_id: String,
    workspace_root: Option<PathBuf>,
    auth_token: Option<String>,
}

impl RunTaskTool {
    pub fn new(
        controller: Arc<dyn AutomatonController>,
        project_id: String,
        workspace_root: Option<PathBuf>,
        auth_token: Option<String>,
    ) -> Self {
        Self {
            controller,
            project_id,
            workspace_root,
            auth_token,
        }
    }
}

#[async_trait]
impl Tool for RunTaskTool {
    fn name(&self) -> &str {
        "run_task"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "run_task".into(),
            description: "Start execution of a single task by the dev-loop engine. Returns immediately; monitor progress via the automaton event stream.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "task_id": { "type": "string" },
                    "model": { "type": "string", "description": "Optional model override" }
                },
                "required": ["task_id"]
            }),
            cache_control: None,
        }
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let task_id = args["task_id"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments("missing 'task_id' argument".into()))?;
        let model = args.get("model").and_then(|v| v.as_str()).map(String::from);

        match self
            .controller
            .run_task(
                &self.project_id,
                task_id,
                self.workspace_root.clone(),
                self.auth_token.clone(),
                model,
                None,
                None,
            )
            .await
        {
            Ok(automaton_id) => Ok(ToolResult::success(
                "run_task",
                format!("Task execution started (automaton_id: {automaton_id}). Monitor via /stream/automaton/{automaton_id}"),
            )),
            Err(e) => Ok(ToolResult::failure("run_task", e)),
        }
    }
}

/// Create all dev-loop control tools for a session context.
pub fn devloop_control_tools(
    controller: Arc<dyn AutomatonController>,
    project_id: String,
    workspace_root: Option<PathBuf>,
    auth_token: Option<String>,
) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(StartDevLoopTool::new(
            controller.clone(),
            project_id.clone(),
            workspace_root.clone(),
            auth_token.clone(),
        )),
        Box::new(PauseDevLoopTool::new(
            controller.clone(),
            project_id.clone(),
        )),
        Box::new(StopDevLoopTool::new(controller.clone(), project_id.clone())),
        Box::new(RunTaskTool::new(
            controller,
            project_id,
            workspace_root,
            auth_token,
        )),
    ]
}
