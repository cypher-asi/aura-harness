use super::{
    build_stub_fix_prompt, classify_build_errors, error_category_guidance, file_ops,
    format_tool_arg_hint, looks_like_compiler_errors, AgentLoopEvent, FileOp, FollowUpSuggestion,
    Path, TaskPhase, TaskPlan, TaskToolExecutor, ToolCallInfo, ToolCallResult,
    MAX_STUB_FIX_ATTEMPTS,
};

pub(super) fn enrich_compiler_output_sync(project_folder: &str, raw_output: &str) -> String {
    if !looks_like_compiler_errors(raw_output) {
        return raw_output.to_string();
    }

    let base_path = Path::new(project_folder);

    let categories = classify_build_errors(raw_output);
    let guidance = error_category_guidance(&categories);
    let refs = crate::verify::parse_error_references(raw_output);
    let api_ref = file_ops::resolve_error_context(base_path, &refs);

    let mut enriched = raw_output.to_string();

    if !guidance.is_empty() {
        enriched.push_str("\n\n## Error Diagnosis & Guidance\n\n");
        enriched.push_str(&guidance);
    }

    if !api_ref.is_empty() {
        enriched.push('\n');
        enriched.push_str(&api_ref);
    }

    enriched
}

impl TaskToolExecutor {
    pub(super) async fn track_file_op(&self, tool_name: &str, input: &serde_json::Value) {
        let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("");
        if path.is_empty() {
            return;
        }
        let op = match tool_name {
            "write_file" => {
                let content = input.get("content").and_then(|v| v.as_str()).unwrap_or("");
                FileOp::Create {
                    path: path.to_string(),
                    content: content.to_string(),
                }
            }
            "edit_file" => FileOp::Modify {
                path: path.to_string(),
                content: String::new(),
            },
            "delete_file" => FileOp::Delete {
                path: path.to_string(),
            },
            _ => return,
        };
        self.tracked_file_ops.lock().await.push(op);
    }

    pub(super) fn enrich_compiler_output(&self, raw_output: &str) -> String {
        enrich_compiler_output_sync(&self.project_folder, raw_output)
    }

    pub(super) async fn handle_task_done(
        &self,
        tc: &ToolCallInfo,
        results: &mut Vec<ToolCallResult>,
        stop: &mut bool,
    ) {
        self.extract_notes_and_follow_ups(tc).await;

        if let Some(error_prompt) = self.check_pervasive_errors().await {
            results.push(ToolCallResult {
                tool_use_id: tc.id.clone(),
                content: error_prompt,
                is_error: true,
                stop_loop: false,
                file_changes: Vec::new(),
            });
            return;
        }

        if let Some(review_prompt) = self.check_self_review().await {
            results.push(ToolCallResult {
                tool_use_id: tc.id.clone(),
                content: review_prompt,
                is_error: true,
                stop_loop: false,
                file_changes: Vec::new(),
            });
            return;
        }

        if let Some(no_write_prompt) = self.check_no_writes().await {
            results.push(ToolCallResult {
                tool_use_id: tc.id.clone(),
                content: no_write_prompt,
                is_error: true,
                stop_loop: false,
                file_changes: Vec::new(),
            });
            return;
        }

        if let Some(stub_prompt) = self.check_stubs_and_reject().await {
            results.push(ToolCallResult {
                tool_use_id: tc.id.clone(),
                content: stub_prompt,
                is_error: true,
                stop_loop: false,
                file_changes: Vec::new(),
            });
        } else {
            results.push(ToolCallResult {
                tool_use_id: tc.id.clone(),
                content: r#"{"status":"completed"}"#.to_string(),
                is_error: false,
                stop_loop: true,
                file_changes: Vec::new(),
            });
            *stop = true;
        }
    }

    pub(super) async fn extract_notes_and_follow_ups(&self, tc: &ToolCallInfo) {
        let task_notes = tc
            .input
            .get("notes")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        {
            let mut n = self.notes.lock().await;
            *n = task_notes;
        }
        if let Some(arr) = tc.input.get("follow_ups").and_then(|v| v.as_array()) {
            let mut fu_lock = self.follow_ups.lock().await;
            for fu in arr {
                let title = fu
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let desc = fu
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                fu_lock.push(FollowUpSuggestion {
                    title,
                    description: desc,
                });
            }
        }
        if let Some(reasoning) = tc.input.get("reasoning").and_then(|v| v.as_array()) {
            let reasoning_text: Vec<String> = reasoning
                .iter()
                .filter_map(|r| r.as_str().map(String::from))
                .collect();
            if !reasoning_text.is_empty() {
                let mut n = self.notes.lock().await;
                n.push_str("\n\nReasoning:\n");
                for r in &reasoning_text {
                    n.push_str(&format!("- {r}\n"));
                }
            }
        }
        if tc
            .input
            .get("no_changes_needed")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            *self.no_changes_needed.lock().await = true;
        }
    }

    async fn check_pervasive_errors(&self) -> Option<String> {
        let outcomes = self.recent_tool_outcomes.lock().await;
        if outcomes.last_command_failed {
            return Some(
                "ERROR: The last run_command failed (non-zero exit code). \
                 Your build or test is broken. Fix the errors before completing the task."
                    .to_string(),
            );
        }
        let min_calls = 6;
        let error_threshold = 0.7;
        if outcomes.total >= min_calls {
            let error_ratio = outcomes.errors as f64 / outcomes.total as f64;
            if error_ratio >= error_threshold {
                return Some(format!(
                    "ERROR: {}/{} recent tool calls returned errors ({:.0}% failure rate). \
                     The task is likely incomplete. Review the errors, fix the underlying \
                     issue, then try completing again.",
                    outcomes.errors,
                    outcomes.total,
                    error_ratio * 100.0,
                ));
            }
        }
        None
    }

    async fn check_self_review(&self) -> Option<String> {
        let unreviewed = self.self_review.lock().await.check_review_needed()?;
        Some(format!(
            "SELF-REVIEW REQUIRED: Before completing, re-read the files you modified \
             to verify correctness:\n{}\n\nCheck: (a) changes match task requirements, \
             (b) no placeholder/stub code remains, (c) no debug code left behind.\n\
             Then call task_done again.",
            unreviewed.join("\n"),
        ))
    }

    async fn check_no_writes(&self) -> Option<String> {
        let ops = self.tracked_file_ops.lock().await;
        if !ops.is_empty() {
            return None;
        }
        let no_changes = *self.no_changes_needed.lock().await;
        if no_changes {
            return None;
        }
        Some(
            "ERROR: You are completing this task but have not made any file changes \
             (write_file, edit_file, or delete_file). Implementation tasks must produce \
             file changes. If this task genuinely requires no file changes, call task_done \
             again with \"no_changes_needed\": true and explain why in the \"notes\" field."
                .to_string(),
        )
    }

    async fn check_stubs_and_reject(&self) -> Option<String> {
        let mut attempts = self.stub_fix_attempts.lock().await;
        if *attempts >= MAX_STUB_FIX_ATTEMPTS {
            return None;
        }
        let base_path = Path::new(&self.project_folder);
        let ops = self.tracked_file_ops.lock().await;
        let stub_reports = file_ops::detect_stub_patterns(base_path, &ops);
        if stub_reports.is_empty() {
            return None;
        }
        *attempts += 1;
        let attempt = *attempts;

        self.emit_text(format!(
            "\n[stub detection] found {} stub(s), requesting fix (attempt {}/{})\n",
            stub_reports.len(),
            attempt,
            MAX_STUB_FIX_ATTEMPTS,
        ));

        Some(build_stub_fix_prompt(&stub_reports))
    }

    pub(super) async fn handle_submit_plan(
        &self,
        tc: &ToolCallInfo,
        results: &mut Vec<ToolCallResult>,
    ) {
        let plan = TaskPlan::from_tool_input(&tc.input);
        match plan.validate() {
            Ok(()) => {
                let context_string = plan.as_context_string();
                {
                    let mut phase = self.task_phase.lock().await;
                    *phase = TaskPhase::Implementing { plan };
                }
                results.push(ToolCallResult {
                    tool_use_id: tc.id.clone(),
                    content: format!(
                        "Plan accepted. Proceeding to implementation.\n\n\
                         YOUR PLAN (reference during implementation):\n{context_string}\n\n\
                         Now implement according to this plan. Start with the most \
                         foundational changes first.",
                    ),
                    is_error: false,
                    stop_loop: false,
                    file_changes: Vec::new(),
                });
            }
            Err(reason) => {
                results.push(ToolCallResult {
                    tool_use_id: tc.id.clone(),
                    content: format!("Plan rejected: {reason}. Revise and resubmit."),
                    is_error: true,
                    stop_loop: false,
                    file_changes: Vec::new(),
                });
            }
        }
    }

    pub(super) fn handle_get_context(&self, tc: &ToolCallInfo, results: &mut Vec<ToolCallResult>) {
        results.push(ToolCallResult {
            tool_use_id: tc.id.clone(),
            content: self.task_context.clone(),
            is_error: false,
            stop_loop: false,
            file_changes: Vec::new(),
        });
    }

    pub(super) fn emit_tool_status(&self, tc: &ToolCallInfo, result: &ToolCallResult) {
        let arg_hint = format_tool_arg_hint(tc);
        let status_str = if result.is_error { "error" } else { "ok" };
        let marker = if arg_hint.is_empty() {
            format!("\n[tool: {} -> {}]\n", tc.name, status_str)
        } else {
            format!("\n[tool: {}({}) -> {}]\n", tc.name, arg_hint, status_str)
        };
        self.emit_text(marker);
    }

    /// Merge tracked executor state (file ops, notes, follow-ups) into a
    /// [`TaskExecutionResult`] so that downstream consumers see real evidence
    /// instead of hardcoded defaults.
    #[allow(clippy::assigning_clones)] // clone_from doesn't work through MutexGuard
    pub async fn merge_into_result(&self, exec: &mut crate::agent_runner::TaskExecutionResult) {
        exec.file_ops = self.tracked_file_ops.lock().await.clone();
        let task_notes = self.notes.lock().await.clone();
        if !task_notes.is_empty() {
            exec.notes = task_notes;
        }
        exec.follow_up_tasks = self.follow_ups.lock().await.clone();
        exec.no_changes_needed = *self.no_changes_needed.lock().await;
        let phase = self.task_phase.lock().await;
        exec.reached_implementing =
            matches!(*phase, crate::planning::TaskPhase::Implementing { .. });
    }

    pub(super) fn emit_text(&self, text: String) {
        if let Some(tx) = &self.event_tx {
            if let Err(e) = tx.try_send(AgentLoopEvent::TextDelta(text)) {
                tracing::warn!("event channel full or closed: {e}");
            }
        }
    }
}
