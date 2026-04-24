use super::safe_transition::safe_transition;
use super::{
    info, topological_sort, warn, Automaton, AutomatonError, AutomatonEvent, DevLoopAutomaton,
    DevLoopConfig, DomainApi, HashMap, HashSet, Schedule, TaskAggregate, TaskDescriptor,
    TaskExecutionResult, TickContext, TickOutcome, COMMIT_SKIPPED_NO_CHANGES,
    STATE_COMPLETED_COUNT, STATE_DONE_IDS, STATE_FAILED_COUNT, STATE_FAILED_IDS,
    STATE_FAILURE_REASONS, STATE_INITIALIZED, STATE_LOOP_FINISHED, STATE_TASK_QUEUE,
    STATE_WORK_LOG,
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
                "no tool executor configured â€” the agent cannot perform file or command operations"
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
        // `safe_transition` guards against the `done → done` rejection
        // observed in production: the agent's `task_done` tool moves the
        // task to `done` server-side first, and this post-execution
        // sync previously surfaced the redundant POST as a WARN.
        if let Err(e) = safe_transition(self.domain.as_ref(), &task.id, "done").await {
            warn!(task_id = %task.id, error = %e, "Failed to sync task done status to backend");
        }

        let mut done_ids: Vec<String> = ctx.state.get(STATE_DONE_IDS).unwrap_or_default();
        done_ids.push(task.id.clone());
        ctx.state.set(STATE_DONE_IDS, &done_ids);

        let completed: u32 = ctx.state.get(STATE_COMPLETED_COUNT).unwrap_or(0) + 1;
        ctx.state.set(STATE_COMPLETED_COUNT, &completed);

        // Build the DoD aggregate BEFORE we move `exec.notes` into
        // the work-log entry / `TaskCompleted` summary: after those
        // moves `exec` is partially dropped and can no longer be
        // borrowed. The aggregate is the only thing `commit_and_push`
        // needs from `exec` (see the commit-skip precheck below).
        let aggregate = TaskAggregate::from_exec(&exec);

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

        commit_and_push(ctx, self.tool_executor.as_ref(), &task.id, &aggregate).await;

        info!(task_id = %task.id, title = %task.title, "Task completed successfully");
    }

    async fn record_task_failure(
        &self,
        ctx: &mut TickContext,
        task: &TaskDescriptor,
        e: AutomatonError,
    ) {
        warn!(task_id = %task.id, error = %e, "Task execution failed");

        // `safe_transition` bridges `ready → in_progress → failed` when
        // the task is still in `ready` (storage rejects the direct
        // `ready → failed` jump; see the note in
        // `aura-os-server/src/handlers/dev_loop.rs`).
        if let Err(te) = safe_transition(self.domain.as_ref(), &task.id, "failed").await {
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
    // Delegate to `safe_transition` so the `failed → ready →
    // in_progress` bridge (used when re-running a previously-failed
    // task) is applied consistently with the success / failure paths.
    // `safe_transition` also skips the `pending → ready` leg when the
    // task is already ready or in_progress, avoiding stray WARNs.
    if let Err(e) = safe_transition(domain, &task.id, "in_progress").await {
        warn!(task_id = %task.id, error = %e, "Failed to transition task to in_progress (continuing anyway)");
    }
}

// ---------------------------------------------------------------------------
// Git mutation plumbing
// ---------------------------------------------------------------------------
//
// Phase 2 (Invariant §1 — Sole External Gateway): all mutating git
// operations (`add`, `commit`, `push`) are now tool calls dispatched
// through the kernel's `ToolExecutor` via the dev-loop's
// `KernelToolGateway`. That gives every invocation a `ToolProposal`
// entry in the record log, full policy enforcement, and a single
// `Command::new("git")` call-site located in `aura-tools/src/git_tool/`.
//
// `git init` is the one exception. Creating a fresh `.git/` directory
// is an infrastructure bootstrap (analogous to `RocksStore::open` —
// already declared in `docs/invariants.md`): it touches no external
// service, cannot leak state across agents, and happens exactly once
// per workspace lifetime. Routing a single `git init` through the
// full kernel/tool pipeline would require surfacing an init tool
// whose sole effect is to create a local directory, which is
// strictly less auditable (noisier allowlist, another permission
// knob) without adding any safety margin. The call below is the
// documented allowance; any other mutating git invocation must go
// through the tool executor.

/// Commit staged changes and push to the Orbit remote if the automaton
/// config includes `git_repo_url`. Called after each successful task
/// completion. The mutating operations (`git add -A`, `git commit --trailer "Made-with: Cursor"`,
/// `git push`) are dispatched through `tool_executor` as kernel-
/// mediated `ToolProposal`s; this function never spawns `git` itself
/// beyond the documented `git init` bootstrap.
pub(crate) async fn commit_and_push(
    ctx: &mut TickContext,
    tool_executor: Option<&std::sync::Arc<dyn aura_agent::types::AgentToolExecutor>>,
    task_id: &str,
    aggregate: &TaskAggregate,
) {
    // DoD precheck: when the per-task aggregate shows zero file
    // changes AND zero verification steps, skip both git_commit and
    // git_commit_push entirely. Runs BEFORE the workspace / git-init
    // checks so callers see a deterministic `CommitSkipped` event
    // regardless of whether the workspace happens to be a git repo.
    // Prevents the "orphan commit" pattern where all `write_file`
    // calls got abandoned by transient 5xx yet the runner still
    // emitted `task_completed`, producing a commit the server-side
    // DoD gate later has to roll back via `git_commit_rolled_back`.
    if aggregate.should_skip_commit() {
        warn!(
            task_id,
            reason = COMMIT_SKIPPED_NO_CHANGES,
            "skipping git_commit: no file changes or verification evidence in per-task aggregate"
        );
        ctx.emit(AutomatonEvent::CommitSkipped {
            task_id: task_id.to_string(),
            reason: COMMIT_SKIPPED_NO_CHANGES.to_string(),
        });
        return;
    }

    let workspace = match ctx.workspace_root.as_ref() {
        Some(ws) => ws.to_string_lossy().to_string(),
        None => return,
    };

    if !aura_agent::git::is_git_repo(&workspace) && !init_git_repo(&workspace, task_id).await {
        return;
    }

    let Some(executor) = tool_executor else {
        warn!(
            task_id,
            "dev-loop has no tool executor; skipping commit/push"
        );
        return;
    };

    // Copy config values out before we reborrow `ctx` mutably to emit
    // events — otherwise the `ctx.config.get(...)` immutable borrow
    // would collide with the `dispatch_git_*(ctx, ...)` calls below.
    let git_repo_url = ctx
        .config
        .get("git_repo_url")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let git_branch = ctx
        .config
        .get("git_branch")
        .and_then(|v| v.as_str())
        .unwrap_or("main")
        .to_string();
    let auth_token = ctx
        .config
        .get("auth_token")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    // Whenever the operator provided both a repo URL and a JWT we
    // bundle commit+push into a single kernel-mediated tool call —
    // cheaper (one policy gate, one record entry) and atomic for the
    // dev-loop's happy path. When either is missing we still attempt
    // a local commit so in-workspace history is preserved.
    if let (Some(repo), Some(jwt)) = (git_repo_url, auth_token) {
        let message = format!("task({task_id}): completed");
        let input = serde_json::json!({
            "message": message,
            "remote_url": &repo,
            "branch": &git_branch,
            "jwt": jwt,
        });
        dispatch_git_commit_push(ctx, executor.as_ref(), task_id, &repo, &git_branch, input).await;
    } else {
        let message = format!("task({task_id}): completed");
        let input = serde_json::json!({ "message": message });
        dispatch_git_commit(ctx, executor.as_ref(), task_id, input).await;
    }
}

async fn dispatch_git_commit(
    ctx: &mut TickContext,
    executor: &dyn aura_agent::types::AgentToolExecutor,
    task_id: &str,
    input: serde_json::Value,
) {
    let tool_use_id = format!("devloop-git-commit-{task_id}");
    let call = aura_agent::types::ToolCallInfo {
        id: tool_use_id,
        name: "git_commit".to_string(),
        input,
    };
    let results = executor.execute(&[call]).await;
    match results.into_iter().next() {
        Some(res) if !res.is_error => {
            let sha = parse_sha(&res.content);
            if let Some(sha) = sha {
                ctx.emit(AutomatonEvent::GitCommitted {
                    task_id: task_id.to_string(),
                    commit_sha: sha,
                });
            } else {
                ctx.emit(AutomatonEvent::GitCommitFailed {
                    task_id: task_id.to_string(),
                    reason: "No changes to commit".to_string(),
                });
            }
        }
        Some(res) => {
            warn!(task_id, error = %res.content, "git_commit tool call failed");
            ctx.emit(AutomatonEvent::GitCommitFailed {
                task_id: task_id.to_string(),
                reason: format!("Commit failed: {}", res.content),
            });
        }
        None => {
            warn!(task_id, "git_commit returned no result");
        }
    }
}

async fn dispatch_git_commit_push(
    ctx: &mut TickContext,
    executor: &dyn aura_agent::types::AgentToolExecutor,
    task_id: &str,
    repo: &str,
    branch: &str,
    input: serde_json::Value,
) {
    let tool_use_id = format!("devloop-git-commit-push-{task_id}");
    let call = aura_agent::types::ToolCallInfo {
        id: tool_use_id,
        name: "git_commit_push".to_string(),
        input,
    };
    let results = executor.execute(&[call]).await;
    match results.into_iter().next() {
        Some(res) if !res.is_error => {
            // The tool returns a successful `ToolResult` even when the
            // push leg failed — the payload carries `pushed: false` and
            // `push_error` in that case. We surface the commit SHA
            // first (so `GitCommitted` is recorded regardless of push
            // status) and then either `GitPushed` or `GitPushFailed`
            // based on the `pushed` flag.
            let parsed = parse_commit_push(&res.content);
            if let Some(sha) = parsed.sha {
                ctx.emit(AutomatonEvent::GitCommitted {
                    task_id: task_id.to_string(),
                    commit_sha: sha,
                });
            } else {
                ctx.emit(AutomatonEvent::GitCommitFailed {
                    task_id: task_id.to_string(),
                    reason: "No changes to commit".to_string(),
                });
            }
            if parsed.pushed {
                ctx.emit(AutomatonEvent::GitPushed {
                    task_id: task_id.to_string(),
                    repo: repo.to_string(),
                    branch: branch.to_string(),
                    commits: parsed.commits,
                });
                info!(
                    task_id,
                    branch = branch,
                    "auto-pushed to orbit via kernel-mediated tool"
                );
            } else {
                let reason = parsed.push_error.unwrap_or_else(|| {
                    "Commit+push: push leg reported no success but no error message".to_string()
                });
                warn!(
                    task_id,
                    branch = branch,
                    %reason,
                    "git_commit_push: commit succeeded, push failed"
                );
                ctx.emit(AutomatonEvent::GitPushFailed {
                    task_id: task_id.to_string(),
                    reason: format!("Commit+push failed: {reason}"),
                });
            }
        }
        Some(res) => {
            warn!(task_id, error = %res.content, "git_commit_push tool call failed");
            ctx.emit(AutomatonEvent::GitPushFailed {
                task_id: task_id.to_string(),
                reason: format!("Commit+push failed: {}", res.content),
            });
        }
        None => {
            warn!(task_id, "git_commit_push returned no result");
        }
    }
}

fn parse_sha(content: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|v| v.get("sha").and_then(|s| s.as_str().map(str::to_string)))
}

struct ParsedCommitPush {
    sha: Option<String>,
    pushed: bool,
    push_error: Option<String>,
    commits: Vec<serde_json::Value>,
}

fn parse_commit_push(content: &str) -> ParsedCommitPush {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(content) else {
        return ParsedCommitPush {
            sha: None,
            pushed: false,
            push_error: None,
            commits: Vec::new(),
        };
    };
    let sha = v.get("sha").and_then(|s| s.as_str().map(str::to_string));
    // Legacy payloads (before the commit/push split) never carried a
    // `pushed` field; their `Ok` arm always implied both commit and
    // push succeeded. Default to `true` when the field is absent so
    // older tool runtimes stay on the success path, and only treat
    // `pushed: false` (the new post-split shape) as a push failure.
    let pushed = v
        .get("pushed")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let push_error = v
        .get("push_error")
        .and_then(|e| e.as_str().map(str::to_string));
    let commits = v
        .get("commits")
        .and_then(|c| c.as_array().cloned())
        .unwrap_or_default();
    ParsedCommitPush {
        sha,
        pushed,
        push_error,
        commits,
    }
}

/// Bootstrap an empty workspace into a git repo on first run.
///
/// Declared exception to Invariant §1 (see
/// `docs/invariants.md` — "Infrastructure bootstrap (`RocksStore::open`,
/// `create_dir_all` for data dirs)"). `git init` creates only
/// a local `.git/` directory, has no remote, and cannot modify
/// external state — the same rationale that lets us open RocksDB
/// without kernel mediation. The allow-list in
/// `scripts/check_invariants.sh` pins this single call-site.
async fn init_git_repo(workspace: &str, task_id: &str) -> bool {
    info!(task_id, %workspace, "Workspace is not a git repo; initializing (bootstrap exception — see docs/invariants.md)");
    let init = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(workspace)
            .output(),
    )
    .await;
    match init {
        Ok(Ok(o)) if o.status.success() => {
            info!(task_id, "git init succeeded");
            true
        }
        Ok(Ok(o)) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            warn!(task_id, %stderr, "git init failed");
            false
        }
        Ok(Err(e)) => {
            warn!(task_id, error = %e, "failed to run git init");
            false
        }
        Err(_) => {
            warn!(task_id, "git init timed out after 30s");
            false
        }
    }
}
