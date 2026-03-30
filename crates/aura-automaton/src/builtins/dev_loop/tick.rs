use super::*;

#[async_trait::async_trait]
impl Automaton for DevLoopAutomaton {
    fn kind(&self) -> &str {
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
        let project_id = &cfg.project_id;

        // ==================================================================
        // 0. Initialize: fetch all tasks, sort, build internal queue
        // ==================================================================
        let initialized: bool = ctx.state.get(STATE_INITIALIZED).unwrap_or(false);
        if !initialized {
            if self.tool_executor.is_none() {
                return Err(AutomatonError::InvalidConfig(
                    "no tool executor configured — the agent cannot perform file or command operations".into(),
                ));
            }

            let tasks = self
                .domain
                .list_tasks(project_id, None, None)
                .await
                .map_err(|e| AutomatonError::DomainApi(e.to_string()))?;

            if tasks.is_empty() {
                info!("No tasks found for project, finishing");
                return self.finish(ctx).await;
            }

            let already_done: Vec<String> = tasks
                .iter()
                .filter(|t| t.status == "done")
                .map(|t| t.id.clone())
                .collect();

            let executable: Vec<&TaskDescriptor> =
                tasks.iter().filter(|t| t.status != "done").collect();

            let sorted =
                topological_sort(&executable.iter().map(|t| (*t).clone()).collect::<Vec<_>>());

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

            return Ok(TickOutcome::Continue);
        }

        // ==================================================================
        // 1. Pick next task from queue
        // ==================================================================
        let mut queue: Vec<String> = ctx.state.get(STATE_TASK_QUEUE).unwrap_or_default();
        let done_ids: Vec<String> = ctx.state.get(STATE_DONE_IDS).unwrap_or_default();
        let done_set: HashSet<&str> = done_ids.iter().map(|s| s.as_str()).collect();

        if queue.is_empty() {
            if self.try_retry_failed(ctx, project_id).await? {
                return Ok(TickOutcome::Continue);
            }
            info!("Task queue empty, finishing loop");
            return self.finish(ctx).await;
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

        if !task.dependencies.is_empty()
            && !task
                .dependencies
                .iter()
                .all(|dep| done_set.contains(dep.as_str()))
        {
            info!(task_id = %task.id, title = %task.title, "Dependencies not yet met, deferring");
            let mut queue: Vec<String> = ctx.state.get(STATE_TASK_QUEUE).unwrap_or_default();
            queue.push(task.id.clone());
            ctx.state.set(STATE_TASK_QUEUE, &queue);
            return Ok(TickOutcome::Continue);
        }

        // ==================================================================
        // 2. Transition to in_progress and execute
        // ==================================================================
        info!(task_id = %task.id, title = %task.title, "Starting task");

        if task.status == "pending" {
            if let Err(e) = self.domain.transition_task(&task.id, "ready", None).await {
                warn!(task_id = %task.id, error = %e, "Failed to transition task to ready");
            }
        }
        if let Err(e) = self
            .domain
            .transition_task(&task.id, "in_progress", None)
            .await
        {
            warn!(task_id = %task.id, error = %e, "Failed to transition task to in_progress (continuing anyway)");
        }

        ctx.emit(AutomatonEvent::TaskStarted {
            task_id: task.id.clone(),
            task_title: task.title.clone(),
        });

        let result = self.execute_task(ctx, &cfg, &task).await;

        // ==================================================================
        // 3. Process result
        // ==================================================================
        match result {
            Ok(exec) => {
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
            Err(e) => {
                warn!(task_id = %task.id, error = %e, "Task execution failed");

                if let Err(te) = self.domain.transition_task(&task.id, "failed", None).await {
                    warn!(task_id = %task.id, error = %te, "Failed to sync task failed status to backend");
                }

                let mut failed_ids: Vec<String> =
                    ctx.state.get(STATE_FAILED_IDS).unwrap_or_default();
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

        Ok(TickOutcome::Continue)
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

/// Commit staged changes and push to the Orbit remote if the automaton config
/// includes `git_repo_url`. Called after each successful task completion.
pub(crate) async fn commit_and_push(ctx: &mut TickContext, task_id: &str) {
    let workspace = match ctx.workspace_root.as_ref() {
        Some(ws) => ws.to_string_lossy().to_string(),
        None => return,
    };

    if !aura_agent::git::is_git_repo(&workspace) {
        info!(task_id, %workspace, "Workspace is not a git repo; initializing");
        let init = tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(&workspace)
            .output()
            .await;
        match init {
            Ok(o) if o.status.success() => {
                info!(task_id, "git init succeeded");
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                warn!(task_id, %stderr, "git init failed");
                return;
            }
            Err(e) => {
                warn!(task_id, error = %e, "failed to run git init");
                return;
            }
        }
    }

    let commit_msg = format!("task({}): completed", task_id);
    let sha = match aura_agent::git::git_commit(&workspace, &commit_msg).await {
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

    let git_repo_url = ctx.config.get("git_repo_url").and_then(|v| v.as_str());
    let git_branch = ctx
        .config
        .get("git_branch")
        .and_then(|v| v.as_str())
        .unwrap_or("main");
    let auth_token = ctx.config.get("auth_token").and_then(|v| v.as_str());

    if let (Some(repo_url), Some(jwt)) = (git_repo_url, auth_token) {
        match aura_agent::git::git_push(&workspace, repo_url, git_branch, jwt).await {
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
}
