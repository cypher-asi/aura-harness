//! [`AutomatonController`] for [`AutomatonBridge`]: the tool-facing
//! dev-loop / task-run surface, fanned out to `dispatch::*`.

use async_trait::async_trait;
use aura_exec_tools::automaton_tools::{AutomatonController, AutomatonRun};
use tracing::info;

use super::kickoff::{AutomatonStart, TaskSpec};
use super::AutomatonBridge;

/// What a start from the tools carries: `run`, with full access and
/// no installed tools, identity or execution profile.
fn tool_start(run: AutomatonRun<'_>) -> AutomatonStart<'_> {
    AutomatonStart {
        project_id: run.project_id,
        workspace_root: run.workspace_root,
        auth_token: run.auth_token,
        model: run.model,
        git_repo_url: run.git_repo_url,
        git_branch: run.git_branch,
        installed_tools: None,
        installed_integrations: None,
        agent_permissions: aura_core_types::AgentPermissions::full_access(),
        aura_org_id: None,
        aura_session_id: None,
        aura_agent_id: None,
        agent_persona: None,
        agent_skills: Vec::new(),
        agent_system_prompt: None,
        execution_profile: None,
    }
}

#[async_trait]
impl AutomatonController for AutomatonBridge {
    async fn start_dev_loop(&self, run: AutomatonRun<'_>) -> Result<String, String> {
        self.start_dev_loop_with_capabilities(tool_start(run)).await
    }

    async fn pause_dev_loop(&self, project_id: &str) -> Result<(), String> {
        let (automaton_id, agent_id) = {
            let entry = self
                .project_handles
                .get(project_id)
                .ok_or_else(|| format!("No running dev loop for project {project_id}"))?;
            let tracked = entry.value();
            if tracked.handle.is_finished() {
                return Err("Dev loop has already finished".into());
            }
            tracked.handle.pause();
            (tracked.automaton_id.clone(), tracked.agent_id)
        };
        // Mirror `stop_dev_loop`'s audit trail. See `pause_by_id` for
        // why the `System::AutomatonLifecycle` write matters.
        self.record_lifecycle_event(agent_id, &automaton_id, "pause_dev_loop")
            .await;
        info!(project_id, automaton_id = %automaton_id, "Dev loop paused");
        Ok(())
    }

    async fn stop_dev_loop(&self, project_id: &str) -> Result<(), String> {
        let (automaton_id, agent_id) = {
            let entry = self
                .project_handles
                .get(project_id)
                .ok_or_else(|| format!("No running dev loop for project {project_id}"))?;
            let tracked = entry.value();
            if tracked.handle.is_finished() {
                let project_id_owned = project_id.to_string();
                drop(entry);
                self.project_handles.remove(&project_id_owned);
                return Err("Dev loop has already finished".into());
            }
            tracked.handle.stop();
            (tracked.automaton_id.clone(), tracked.agent_id)
        };
        self.project_handles.remove(project_id);
        self.record_lifecycle_event(agent_id, &automaton_id, "stop_dev_loop")
            .await;
        info!(project_id, automaton_id = %automaton_id, "Dev loop stopped");
        Ok(())
    }

    async fn run_task(&self, task_id: &str, run: AutomatonRun<'_>) -> Result<String, String> {
        let task = TaskSpec {
            task_id,
            prior_failure: None,
            work_log: Vec::new(),
        };
        self.run_task_with_capabilities(tool_start(run), task).await
    }
}
