use super::{
    info, topological_sort, warn, Automaton, AutomatonError, AutomatonEvent, DevLoopAutomaton,
    DevLoopConfig, DomainApi, HashMap, HashSet, Schedule, TaskDescriptor, TaskExecutionResult,
    TickContext, TickOutcome, STATE_COMPLETED_COUNT, STATE_DONE_IDS, STATE_FAILED_COUNT,
    STATE_FAILED_IDS, STATE_FAILURE_REASONS, STATE_INITIALIZED, STATE_LOOP_FINISHED,
    STATE_TASK_QUEUE, STATE_WORK_LOG,
};

#[async_trait::async_trait]
impl Automaton for DevLoopAutomaton {
    fn kind(&self) -> &'static str {
        "dev-loop"
    }

    fn default_schedule(&self) -> Schedule {
        Schedule::Continuous
    }

    async fn on_install(&self, ctx: &TickContext) -> Result<(), AutomatonError> {
        let cfg = DevLoopConfig::from_json(&ctx.config)?;
        info!(project_id = %cfg.project_id, "Dev loop automaton installed");
        ctx.emit(AutomatonEvent::LogLine {
            message: format!("dev loop starting for project {}", cfg.project_id),
        });
        Ok(())
    }

    async fn tick(&self, ctx: &mut TickContext) -> Result<TickOutcome, AutomatonError> {
        if ctx.is_cancelled() {
            return Ok(TickOutcome::Done);
        }

        let cfg = DevLoopConfig::from_json(&ctx.config)?;
        let initialized: bool = ctx.state.get(STATE_INITIALIZED).unwrap_or(false);

        if !initialized {
            return self.initialize_queue(ctx, &cfg).await;
        }

        self.process_next_task(ctx, &cfg).await
    }

    async fn on_stop(&self, ctx: &TickContext) -> Result<(), AutomatonError> {
        let already_finished: bool = ctx.state.get(STATE_LOOP_FINISHED).unwrap_or(false);
        if !already_finished {
            let completed: u32 = ctx.state.get(STATE_COMPLETED_COUNT).unwrap_or(0);
            let failed: u32 = ctx.state.get(STATE_FAILED_COUNT).unwrap_or(0);
            ctx.emit(AutomatonEvent::LoopFinished {
                outcome: "stopped".into(),
                completed_count: completed,
                failed_count: failed,
            });
        }
        Ok(())
    }
}

impl DevLoopAutomaton {
    async fn initialize_queue(
        &self,
        ctx: &mut TickContext,
        cfg: &DevLoopConfig,
    ) -> Result<TickOutcome, AutomatonError> {
        if self.tool_executor.is_none() {
            return Err(AutomatonError::InvalidConfig(
                "no tool executor configured — the agent cannot perform file or command operations"
                    .into(),
            ));
        }

        let tasks = self
            .domain
            .list_tasks(&cfg.project_id, None, None)
            .await
            .map_err(|e| AutomatonError::DomainApi(e.to_string()))?;

        if tasks.is_empty() {
            info!("No tasks found for project, finishing");
            return self.finish(ctx);
        }

        let already_done: Vec<String> = tasks
            .iter()
            .filter(|t| t.status == "done")
            .map(|t| t.id.clone())
            .collect();

        let executable: Vec<&TaskDescriptor> =
            tasks.iter().filter(|t| t.status != "done").collect();

        let sorted = topological_sort(&executable.iter().map(|t| (*t).clone()).collect::<Vec<_>>());

        info!(
            total = tasks.len(),
            already_done = already_done.len(),
            to_execute = sorted.len(),
            "Task queue initialized"
        );

        ctx.state.set(STATE_TASK_QUEUE, &sorted);
        ctx.state.set(STATE_DONE_IDS, &already_done);
        ctx.state.set::<Vec<String>>(STATE_FAILED_IDS, &vec![]);
        ctx.state.set(STATE_INITIALIZED, &true);

        ctx.emit(AutomatonEvent::LogLine {
            message: format!(
                "Dev loop ready: {} tasks to execute ({} already done)",
                sorted.len(),
                already_done.len()
            ),
        });

        Ok(TickOutcome::Continue)
    }

    async fn process_next_task(
        &self,
        ctx: &mut TickContext,
        cfg: &DevLoopConfig,
    ) -> Result<TickOutcome, AutomatonError> {
        let mut queue: Vec<String> = ctx.state.get(STATE_TASK_QUEUE).unwrap_or_default();
        let done_ids: Vec<String> = ctx.state.get(STATE_DONE_IDS).unwrap_or_default();
        let done_set: HashSet<&str> = done_ids.iter().map(std::string::String::as_str).collect();

        if queue.is_empty() {
            if self.try_retry_failed(ctx, &cfg.project_id).await? {
                return Ok(TickOutcome::Continue);
            }
            info!("Task queue empty, finishing loop");
            return self.finish(ctx);
        }

        let task_id = queue.remove(0);
        ctx.state.set(STATE_TASK_QUEUE, &queue);

        let task = match self.domain.get_task(&task_id, None).await {
            Ok(t) => t,
            Err(e) => {
                warn!(task_id = %task_id, error = %e, "Failed to fetch task, skipping");
                return Ok(TickOutcome::Continue);
            }
        };

        if !deps_satisfied(&task, &done_set) {
            let mut queue: Vec<String> = ctx.state.get(STATE_TASK_QUEUE).unwrap_or_default();
            queue.push(task.id.clone());
            ctx.state.set(STATE_TASK_QUEUE, &queue);
            return Ok(TickOutcome::Continue);
        }

        self.run_and_record_task(ctx, cfg, &task).await
    }

    async fn run_and_record_task(
        &self,
        ctx: &mut TickContext,
        cfg: &DevLoopConfig,
        task: &TaskDescriptor,
    ) -> Result<TickOutcome, AutomatonError> {
        info!(task_id = %task.id, title = %task.title, "Starting task");

        transition_to_in_progress(self.domain.as_ref(), task).await;

        ctx.emit(AutomatonEvent::TaskStarted {
            task_id: task.id.clone(),
            task_title: task.title.clone(),
        });

        let result = self.execute_task(ctx, cfg, task).await;

        match result {
            Ok(exec) => self.record_task_success(ctx, task, exec).await,
            Err(e) => self.record_task_failure(ctx, task, e).await,
        }

        Ok(TickOutcome::Continue)
    }

    async fn record_task_success(
        &self,
        ctx: &mut TickContext,
        task: &TaskDescriptor,
        exec: TaskExecutionResult,
    ) {
        if let Err(e) = self.domain.transition_task(&task.id, "done", None).await {
            warn!(task_id = %task.id, error = %e, "Failed to sync task done status to backend");
        }

        let mut done_ids: Vec<String> = ctx.state.get(STATE_DONE_IDS).unwrap_or_default();
        done_ids.push(task.id.clone());
        ctx.state.set(STATE_DONE_IDS, &done_ids);

        let completed: u32 = ctx.state.get(STATE_COMPLETED_COUNT).unwrap_or(0) + 1;
        ctx.state.set(STATE_COMPLETED_COUNT, &completed);

        let mut work_log: Vec<String> = ctx.state.get(STATE_WORK_LOG).unwrap_or_default();
        work_log.push(format!(
            "Task (completed): {}\nNotes: {}",
            task.title, exec.notes
        ));
        ctx.state.set(STATE_WORK_LOG, &work_log);

        ctx.emit(AutomatonEvent::TaskCompleted {
            task_id: task.id.clone(),
            summary: exec.notes,
        });
        ctx.emit(AutomatonEvent::TokenUsage {
            input_tokens: exec.input_tokens,
            output_tokens: exec.output_tokens,
        });

        commit_and_push(ctx, &task.id).await;

        info!(task_id = %task.id, title = %task.title, "Task completed successfully");
    }

    async fn record_task_failure(
        &self,
        ctx: &mut TickContext,
        task: &TaskDescriptor,
        e: AutomatonError,
    ) {
        warn!(task_id = %task.id, error = %e, "Task execution failed");

        if let Err(te) = self.domain.transition_task(&task.id, "failed", None).await {
            warn!(task_id = %task.id, error = %te, "Failed to sync task failed status to backend");
        }

        let mut failed_ids: Vec<String> = ctx.state.get(STATE_FAILED_IDS).unwrap_or_default();
        failed_ids.push(task.id.clone());
        ctx.state.set(STATE_FAILED_IDS, &failed_ids);

        let mut failure_reasons: HashMap<String, String> =
            ctx.state.get(STATE_FAILURE_REASONS).unwrap_or_default();
        failure_reasons.insert(task.id.clone(), e.to_string());
        ctx.state.set(STATE_FAILURE_REASONS, &failure_reasons);

        let failed: u32 = ctx.state.get(STATE_FAILED_COUNT).unwrap_or(0) + 1;
        ctx.state.set(STATE_FAILED_COUNT, &failed);

        let mut work_log: Vec<String> = ctx.state.get(STATE_WORK_LOG).unwrap_or_default();
        work_log.push(format!("Task (failed): {}\nReason: {e}", task.title));
        ctx.state.set(STATE_WORK_LOG, &work_log);

        ctx.emit(AutomatonEvent::TaskFailed {
            task_id: task.id.clone(),
            reason: e.to_string(),
        });
    }
}

fn deps_satisfied(task: &TaskDescriptor, done_set: &HashSet<&str>) -> bool {
    task.dependencies.is_empty()
        || task
            .dependencies
            .iter()
            .all(|dep| done_set.contains(dep.as_str()))
}

async fn transition_to_in_progress(domain: &dyn DomainApi, task: &TaskDescriptor) {
    if task.status == "pending" {
        if let Err(e) = domain.transition_task(&task.id, "ready", None).await {
            warn!(task_id = %task.id, error = %e, "Failed to transition task to ready");
        }
    }
    if let Err(e) = domain.transition_task(&task.id, "in_progress", None).await {
        warn!(task_id = %task.id, error = %e, "Failed to transition task to in_progress (continuing anyway)");
    }
}

/// Commit staged changes and push to the Orbit remote if the automaton config
/// includes `git_repo_url`. Called after each successful task completion.
pub async fn commit_and_push(ctx: &mut TickContext, task_id: &str) {
    let workspace = match ctx.workspace_root.as_ref() {
        Some(ws) => ws.to_string_lossy().to_string(),
        None => return,
    };

    if !aura_agent::git::is_git_repo(&workspace) && !init_git_repo(&workspace, task_id).await {
        return;
    }

    let sha = match aura_agent::git::git_commit(&workspace, &format!("task({task_id}): completed"))
        .await
    {
        Ok(Some(sha)) => sha,
        Ok(None) => {
            ctx.emit(AutomatonEvent::GitCommitFailed {
                task_id: task_id.to_string(),
                reason: "No changes to commit".to_string(),
            });
            return;
        }
        Err(e) => {
            warn!(task_id, error = %e, "auto-commit after task completion failed");
            ctx.emit(AutomatonEvent::GitCommitFailed {
                task_id: task_id.to_string(),
                reason: format!("Commit failed: {e}"),
            });
            return;
        }
    };

    ctx.emit(AutomatonEvent::GitCommitted {
        task_id: task_id.to_string(),
        commit_sha: sha,
    });

    push_to_orbit(ctx, task_id, &workspace).await;
}

async fn init_git_repo(workspace: &str, task_id: &str) -> bool {
    info!(task_id, %workspace, "Workspace is not a git repo; initializing");
    let init = tokio::process::Command::new("git")
        .args(["init"])
        .current_dir(workspace)
        .output()
        .await;
    match init {
        Ok(o) if o.status.success() => {
            info!(task_id, "git init succeeded");
            true
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            warn!(task_id, %stderr, "git init failed");
            false
        }
        Err(e) => {
            warn!(task_id, error = %e, "failed to run git init");
            false
        }
    }
}

async fn push_to_orbit(ctx: &mut TickContext, task_id: &str, workspace: &str) {
    let git_repo_url = ctx.config.get("git_repo_url").and_then(|v| v.as_str());
    let git_branch = ctx
        .config
        .get("git_branch")
        .and_then(|v| v.as_str())
        .unwrap_or("main");
    let auth_token = ctx.config.get("auth_token").and_then(|v| v.as_str());

    let (Some(repo_url), Some(jwt)) = (git_repo_url, auth_token) else {
        return;
    };

    match aura_agent::git::git_push(workspace, repo_url, git_branch, jwt).await {
        Ok(commits) => {
            let commit_values: Vec<serde_json::Value> = commits
                .iter()
                .map(|c| serde_json::json!({"sha": c.sha, "message": c.message}))
                .collect();
            ctx.emit(AutomatonEvent::GitPushed {
                task_id: task_id.to_string(),
                repo: repo_url.to_string(),
                branch: git_branch.to_string(),
                commits: commit_values,
            });
            info!(task_id, branch = git_branch, "auto-pushed to orbit");
        }
        Err(e) => {
            warn!(task_id, error = %e, "auto-push to orbit failed");
            ctx.emit(AutomatonEvent::GitPushFailed {
                task_id: task_id.to_string(),
                reason: format!("Push failed: {e}"),
            });
        }
    }
}
