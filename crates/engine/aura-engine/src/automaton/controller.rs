//! [`AutomatonController`] for [`AutomatonBridge`]: the tool-facing
//! dev-loop / task-run surface, fanned out to `dispatch::*`.

use std::path::PathBuf;

use async_trait::async_trait;
use aura_exec_tools::automaton_tools::AutomatonController;
use tracing::info;

use super::AutomatonBridge;

#[async_trait]
impl AutomatonController for AutomatonBridge {
    async fn start_dev_loop(
        &self,
        project_id: &str,
        workspace_root: Option<PathBuf>,
        auth_token: Option<String>,
        model: Option<String>,
        git_repo_url: Option<String>,
        git_branch: Option<String>,
    ) -> Result<String, String> {
        self.start_dev_loop_with_capabilities(
            project_id,
            workspace_root,
            auth_token,
            model,
            git_repo_url,
            git_branch,
            None,
            None,
            aura_core_types::AgentPermissions::full_access(),
            None,
            None,
            None,
            None,
            Vec::new(),
            None,
            None,
        )
        .await
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

    async fn run_task(
        &self,
        project_id: &str,
        task_id: &str,
        workspace_root: Option<PathBuf>,
        auth_token: Option<String>,
        model: Option<String>,
        git_repo_url: Option<String>,
        git_branch: Option<String>,
    ) -> Result<String, String> {
        self.run_task_with_capabilities(
            project_id,
            task_id,
            workspace_root,
            auth_token,
            model,
            git_repo_url,
            git_branch,
            None,
            None,
            aura_core_types::AgentPermissions::full_access(),
            None,
            Vec::new(),
            None,
            None,
            None,
            None,
            Vec::new(),
            None,
            None,
        )
        .await
    }
}
