//! Hooks the tool layer calls out through.
//!
//! Tools in `aura-tools` reach agents, subagents, background processes and
//! automatons only through these traits; the engine and the surfaces
//! implement them. Keeping them here lets an implementor depend on the
//! seam without depending on the tool catalog.

use async_trait::async_trait;
use aura_core_types::{ActionId, AgentId, ProcessId, SubagentDispatchRequest, SubagentResult};
use std::path::{Path, PathBuf};

/// Hook invoked by the `send_to_agent` / `agent_lifecycle` / `delegate_task`
/// tools to actually affect the target agent. Kept as a trait so the
/// permission gate can be tested without wiring a real kernel writer.
#[async_trait]
pub trait AgentControlHook: Send + Sync {
    /// Deliver a user-message-shaped payload to `target`.
    ///
    /// `model` is the **caller's** resolved model id (from
    /// `ToolContext::caller_model_id`). It is forwarded so the
    /// target's turn runs on a real model: cross-agent recipients
    /// often have no server-side configured model (the UI sends the
    /// model per-turn from client state), so omitting it leaves the
    /// recipient's harness session with an empty model and the turn
    /// fails with "model name must not be empty". `None` preserves the
    /// legacy null-model wire value for callers without a known model.
    async fn deliver_message(
        &self,
        target_agent_id: &str,
        parent_agent_id: Option<&str>,
        originating_user_id: Option<&str>,
        project_id: Option<&str>,
        content: &str,
        attachments: Option<serde_json::Value>,
        model: Option<&str>,
    ) -> Result<(), String>;

    /// Apply a lifecycle transition to `target`.
    async fn lifecycle(
        &self,
        target_agent_id: &str,
        parent_agent_id: Option<&str>,
        originating_user_id: Option<&str>,
        action: &str,
    ) -> Result<(), String>;

    /// Emit a Delegate-tagged task to `target`.
    ///
    /// `model` mirrors [`Self::deliver_message`]: the caller's resolved
    /// model id so the delegated turn runs on a real model.
    async fn delegate_task(
        &self,
        target_agent_id: &str,
        parent_agent_id: Option<&str>,
        originating_user_id: Option<&str>,
        task: &str,
        context: Option<&serde_json::Value>,
        model: Option<&str>,
    ) -> Result<(), String>;
}

/// Hook used by read-only cross-agent tools. Kept as a trait so the gate is
/// testable without a kernel.
#[async_trait]
pub trait AgentReadHook: Send + Sync {
    /// Return the latest `session_ready` / `assistant_message_end`
    /// snapshot for `target`, plus the agent's `Identity` + `permissions`.
    async fn snapshot(&self, target_agent_id: &str) -> Result<serde_json::Value, String>;

    /// Return agents visible to the current caller, optionally narrowed by org.
    async fn list_agents(&self, org_id: Option<&str>) -> Result<serde_json::Value, String>;
}

/// Hook invoked by the `task` tool to run a foreground subagent.
///
/// The trait intentionally uses only `aura-core` data types so `aura-tools`
/// remains independent from `aura-runtime`, `aura-agent` event types, and
/// transport protocols.
#[async_trait]
pub trait SubagentDispatchHook: Send + Sync {
    async fn dispatch(&self, request: SubagentDispatchRequest) -> Result<SubagentResult, String>;
}

/// Told about each file a tool call is about to change and again once the
/// call is done, so a surface can checkpoint the file and show the change.
///
/// Only tools that name the file they change (see `Tool::changed_path` in `aura-tools`)
/// are reported; `run_command` and other tools with side effects the
/// executor can't see are not.
#[async_trait]
pub trait FileChangeHook: Send + Sync {
    /// `path` (resolved inside the sandbox) is about to be changed by the
    /// call `tool_use_id`.
    async fn before_change(&self, path: &Path, tool_use_id: Option<&str>);

    /// The call that [`Self::before_change`] announced is done; `succeeded`
    /// is whether the tool reported success.
    async fn after_change(&self, path: &Path, tool_use_id: Option<&str>, succeeded: bool);
}

/// A command `run_command` started in the background, with its output
/// going to `log_path`.
pub struct BackgroundProcess {
    pub process_id: ProcessId,
    pub child: std::process::Child,
    /// The command as shown to the user.
    pub command: String,
    pub cwd: PathBuf,
    pub log_path: PathBuf,
    pub action_id: Option<ActionId>,
    pub tool_use_id: Option<String>,
}

/// Tracks background commands until they exit and reports the result to
/// the agent that started them.
pub trait BackgroundProcessHook: Send + Sync {
    /// Directory background command logs are written to.
    fn log_dir(&self) -> PathBuf;

    /// Take over `process` for `agent_id`.
    ///
    /// # Errors
    /// When the process can't be tracked, after killing it.
    fn adopt(&self, agent_id: AgentId, process: BackgroundProcess) -> Result<(), String>;

    /// Background commands still running, of `agent_id` or of every agent.
    fn running(&self, agent_id: Option<AgentId>) -> Vec<BackgroundJob> {
        let _ = agent_id;
        Vec::new()
    }

    /// Stop the running background command `id` (its process id in hex)
    /// and every process it started. Its owner hears that it was killed.
    ///
    /// # Errors
    /// When no such command is running or it can't be stopped.
    fn kill(&self, id: &str) -> Result<(), String> {
        Err(format!("background command {id} can't be stopped here"))
    }
}

/// A running background command, as [`BackgroundProcessHook::running`]
/// lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundJob {
    /// Process id in hex, as `run_command` reported it
    pub id: String,
    pub agent_id: AgentId,
    /// The command as shown to the user
    pub command: String,
    pub pid: Option<u32>,
    pub log_path: PathBuf,
    /// When it started, in Unix milliseconds
    pub started_ms: u64,
}

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
