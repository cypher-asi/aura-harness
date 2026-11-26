//! What a dev-loop or task-run kickoff carries from its start request
//! to the installed automaton.

use std::path::PathBuf;

use aura_core_protocol::{AgentPersona, ModelSelection};
use aura_core_types::AgentPermissions;

use super::RunIdentity;

/// One kickoff's validated request fields, shared by the dev-loop and
/// task-run start paths.
pub(super) struct Kickoff<'a> {
    pub(super) project_id: &'a str,
    pub(super) model: String,
    pub(super) auth_token: Option<String>,
    pub(super) git_repo_url: Option<String>,
    pub(super) git_branch: Option<String>,
    pub(super) aura_org_id: Option<String>,
    pub(super) aura_session_id: Option<String>,
    pub(super) aura_agent_id: Option<String>,
    pub(super) agent_persona: Option<AgentPersona>,
    pub(super) agent_skills: Vec<String>,
    pub(super) agent_system_prompt: Option<String>,
    pub(super) execution_profile: Option<ModelSelection>,
}

/// The workspace and tool surface a kickoff installs its kernel with.
pub(super) struct KickoffTools {
    pub(super) workspace_root: Option<PathBuf>,
    pub(super) installed_tools: Option<Vec<aura_core_protocol::InstalledTool>>,
    pub(super) installed_integrations: Option<Vec<aura_core_protocol::InstalledIntegration>>,
    pub(super) agent_permissions: AgentPermissions,
}

/// The task a task-run kickoff executes.
pub(super) struct TaskSpec<'a> {
    pub(super) task_id: &'a str,
    pub(super) prior_failure: Option<String>,
    pub(super) work_log: Vec<String>,
}

impl Kickoff<'_> {
    /// The run's model and identity. `project_id` doubles as
    /// `aura_project_id` on the chat / task-extract paths, so the same
    /// value is used here to keep the dev-loop / task-run wire shape
    /// symmetric with chat.
    pub(super) fn identity(&self) -> RunIdentity<'_> {
        RunIdentity {
            model: &self.model,
            auth_token: self.auth_token.as_deref(),
            aura_org_id: self.aura_org_id.as_deref(),
            aura_session_id: self.aura_session_id.as_deref(),
            aura_agent_id: self.aura_agent_id.as_deref(),
            aura_project_id: Some(self.project_id),
        }
    }

    /// The JSON config blob a dev-loop automaton is installed with.
    pub(super) fn dev_loop_config(&self) -> serde_json::Value {
        serde_json::json!({
            "project_id": self.project_id,
            "model": self.model,
            "git_repo_url": self.git_repo_url,
            "git_branch": self.git_branch,
            "auth_token": self.auth_token.as_deref(),
            // PR B: typed identity wire fields surface to the
            // automaton via the same JSON config blob the rest of the
            // dev-loop kickoff already uses. `DevLoopConfig::from_json`
            // parses them back out and threads them into
            // `AgenticTaskParams::agent`. `null` / `[]` defaults keep
            // the assembled prompt byte-identical with PR A until
            // aura-os populates these fields in PR C.
            "agent_identity": self.agent_persona,
            "agent_skills": self.agent_skills,
            "agent_system_prompt": self.agent_system_prompt,
        })
    }

    /// The JSON config blob a task-run automaton is installed with.
    pub(super) fn task_config(&self, task: &TaskSpec<'_>) -> serde_json::Value {
        serde_json::json!({
            "project_id": self.project_id,
            "task_id": task.task_id,
            "model": self.model,
            "git_repo_url": self.git_repo_url,
            "git_branch": self.git_branch,
            "auth_token": self.auth_token.as_deref(),
            "prior_failure": task.prior_failure,
            "work_log": task.work_log,
            // PR B: same wire-field threading as `start_dev_loop_with_capabilities`.
            // Single-task kickoffs go through the same prompt assembly,
            // so they pick up identity / skills / operator system
            // prompt the same way once PR C populates them upstream.
            "agent_identity": self.agent_persona,
            "agent_skills": self.agent_skills,
            "agent_system_prompt": self.agent_system_prompt,
        })
    }
}

/// The trimmed, non-empty `model`, or the `missing model` error for a
/// `request` (e.g. `"dev loop start request"`).
pub(super) fn require_model(model: Option<&str>, request: &str) -> Result<String, String> {
    model
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            format!("missing model — {request} must include an explicit model identifier")
        })
        .map(ToString::to_string)
}
