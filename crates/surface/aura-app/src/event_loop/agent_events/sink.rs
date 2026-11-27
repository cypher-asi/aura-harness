//! The turn event sink that forwards an agent's events to the UI.

use super::*;

pub(super) const DELTA_FLUSH: Duration = Duration::from_millis(33);

/// How long streamed text of an agent whose chat is not on screen may
/// wait. Nobody watches it arrive, and every command still costs the UI
/// its handling.
pub(super) const HIDDEN_DELTA_FLUSH: Duration = Duration::from_millis(200);

/// Which streamed block buffered text belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DeltaKind {
    Text,
    Thinking,
}

/// Streamed text not yet sent to the UI.
pub(super) struct PendingDelta {
    pub(super) kind: DeltaKind,
    pub(super) text: String,
    pub(super) flush_at: Instant,
}

/// Sink implementation that drains [`AgentLoopEvent`]s from the agent
/// loop and fans them out to the TUI as [`UiCommand`]s while maintaining
/// the streaming / thinking state machine the Ratatui layer depends on.
pub(super) struct UiCommandSink {
    pub(super) ui: AgentUi,
    pub(super) state: ForwarderState,
    pub(super) pending: Option<PendingDelta>,
    /// Tool calls shown in the chat that have no result yet
    pub(super) open_tools: Vec<String>,
    pub(super) cost_cap: Option<CostCap>,
    pub(super) plan: TurnPlan,
    /// Tokens used by the turn's finished model calls
    pub(super) turn_tokens: u64,
}

impl UiCommandSink {
    /// Show the plan `create_plan` wrote, as its `output` names it.
    async fn plan_written(&mut self, output: &str) {
        let Some(path) = plan_path_from_output(output) else {
            return;
        };
        self.state.plan_path = Some(path.to_string());
        // A new plan is not the one being built.
        self.plan.building = None;
        self.plan.tasks.set_plan(self.plan.agent, path).await;
        let Some(view) = plan_view(&self.plan.workspace, path) else {
            return;
        };
        let note = format!(
            "Plan ready: {} ({path}). Keep chatting to revise it, or say \"implement\" (or run /build) to build it.",
            view.name
        );
        self.plan
            .tasks
            .show_plan(&self.plan.workspace, view.clone())
            .await;
        self.ui
            .send_if_shown(UiCommand::SetPlans(plan_views(&self.plan.workspace)))
            .await;
        self.ui.send_if_shown(UiCommand::SetPlan(Some(view))).await;
        self.ui
            .send(UiCommand::ShowMessage(MessageData::new(
                MessageRole::System,
                note,
            )))
            .await;
    }

    /// Mirror `todos` into the plan being built, and show it.
    async fn sync_plan(&self, todos: &[TodoItem]) {
        let Some(path) = &self.plan.building else {
            return;
        };
        match sync_todo_list(&self.plan.workspace.join(path), todos) {
            Ok(false) => {}
            Ok(true) => {
                if let Some(view) = plan_view(&self.plan.workspace, path) {
                    self.plan
                        .tasks
                        .show_plan(&self.plan.workspace, view.clone())
                        .await;
                    self.ui.send_if_shown(UiCommand::SetPlan(Some(view))).await;
                }
            }
            Err(e) => warn!(%path, error = %e, "Could not update the plan's todos"),
        }
    }

    /// Add streamed text to the buffer, sending what it held first when
    /// that belongs to the other block.
    async fn buffer_delta(&mut self, kind: DeltaKind, text: String) {
        if self.pending.as_ref().is_some_and(|p| p.kind != kind) {
            self.flush_delta().await;
        }
        match &mut self.pending {
            Some(pending) => pending.text.push_str(&text),
            None => {
                let wait = if self.ui.maybe_on_screen() {
                    DELTA_FLUSH
                } else {
                    HIDDEN_DELTA_FLUSH
                };
                self.pending = Some(PendingDelta {
                    kind,
                    text,
                    flush_at: Instant::now() + wait,
                });
            }
        }
    }

    pub(super) async fn flush_delta(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let command = match pending.kind {
            DeltaKind::Text => UiCommand::AppendText(pending.text),
            DeltaKind::Thinking => UiCommand::AppendThinking(pending.text),
        };
        self.ui.send(command).await;
    }

    async fn finish_thinking_if_active(&mut self) {
        if self.state.thinking_active {
            self.flush_delta().await;
            self.ui.send(UiCommand::FinishThinking).await;
            self.state.thinking_active = false;
        }
    }

    /// End the reply message being streamed, so text that follows a tool
    /// call, more thinking or the end of the model call starts a new
    /// message below them.
    async fn finish_streaming_if_active(&mut self) {
        if self.state.streaming_active {
            self.flush_delta().await;
            self.ui.send(UiCommand::FinishStreaming).await;
            self.state.streaming_active = false;
        }
    }

    async fn enforce_cost_cap(&mut self) {
        let Some(cap) = &self.cost_cap else {
            return;
        };
        if self.state.turn_cost_usd <= cap.limit_usd || cap.stop.is_cancelled() {
            return;
        }
        cap.stop.cancel();
        let message = format!(
            "Stopping this turn: it has cost about ${:.2}, over the ${:.2} limit \
             (AURA_TUI_MAX_TURN_COST_USD)",
            self.state.turn_cost_usd, cap.limit_usd
        );
        self.ui.send(UiCommand::ShowWarning(message)).await;
    }
}

#[async_trait]
impl TurnEventSink for UiCommandSink {
    async fn on_thinking_delta(&mut self, text: String) {
        if !self.state.thinking_active {
            self.finish_streaming_if_active().await;
            self.ui.send(UiCommand::StartThinking).await;
            self.state.thinking_active = true;
        }
        self.buffer_delta(DeltaKind::Thinking, text).await;
    }

    async fn on_thinking_complete(&mut self) {
        self.finish_thinking_if_active().await;
    }

    async fn on_text_delta(&mut self, text: String) {
        self.finish_thinking_if_active().await;
        if !self.state.streaming_active {
            self.ui.send(UiCommand::StartStreaming).await;
            self.state.streaming_active = true;
        }
        self.state.had_text = true;
        self.buffer_delta(DeltaKind::Text, text).await;
    }

    async fn on_tool_start(&mut self, id: String, name: String) {
        self.finish_thinking_if_active().await;
        self.finish_streaming_if_active().await;
        self.plan.tasks.touch(self.plan.agent).await;
        self.open_tools.push(id.clone());
        self.ui
            .send(UiCommand::ShowTool(ToolData {
                id,
                name,
                args: String::new(),
            }))
            .await;
    }

    async fn on_tool_input_snapshot(&mut self, id: String, _name: String, input: String) {
        self.ui
            .send(UiCommand::UpdateToolArgs { id, args: input })
            .await;
    }

    async fn on_tool_result(
        &mut self,
        tool_use_id: String,
        tool_name: String,
        content: String,
        is_error: bool,
        _image: Option<aura_core_types::ToolResultImage>,
    ) {
        self.open_tools.retain(|id| *id != tool_use_id);
        let plan_output =
            (!is_error && tool_name == CREATE_PLAN_TOOL_NAME).then(|| content.clone());
        self.ui
            .send(UiCommand::CompleteTool {
                id: tool_use_id,
                result: content,
                success: !is_error,
            })
            .await;
        if let Some(output) = plan_output {
            self.plan_written(&output).await;
        }
    }

    async fn on_tool_call_retrying(
        &mut self,
        tool_use_id: String,
        _tool_name: String,
        attempt: u32,
        max_attempts: u32,
        _delay_ms: u64,
        reason: String,
    ) {
        debug!(tool_id = %tool_use_id, %reason, "Tool call retrying");
        self.ui
            .send(UiCommand::ToolRetrying {
                id: tool_use_id,
                attempt,
                max_attempts,
            })
            .await;
    }

    async fn on_tool_call_failed(
        &mut self,
        tool_use_id: String,
        _tool_name: String,
        reason: String,
    ) {
        self.open_tools.retain(|id| *id != tool_use_id);
        self.ui
            .send(UiCommand::CompleteTool {
                id: tool_use_id,
                result: reason,
                success: false,
            })
            .await;
    }

    async fn on_stream_reset(
        &mut self,
        reason: String,
        _text_bytes: usize,
        _thinking_bytes: usize,
    ) {
        debug!(reason = %reason, "Stream reset received");
        self.finish_streaming_if_active().await;
        self.finish_thinking_if_active().await;
        self.state.had_text = false;
        // The model starts its response over, so calls it was still
        // writing will never get a result.
        for id in std::mem::take(&mut self.open_tools) {
            self.ui
                .send(UiCommand::CompleteTool {
                    id,
                    result: reason.clone(),
                    success: false,
                })
                .await;
        }
    }

    async fn on_warning(&mut self, message: String) {
        self.ui.send(UiCommand::ShowWarning(message)).await;
    }

    async fn on_error(&mut self, code: String, message: String, _recoverable: bool) {
        if code == ERROR_UNAUTHORIZED || code == ERROR_INSUFFICIENT_CREDITS {
            self.state.credentials_rejected = true;
        }
        self.ui.send(UiCommand::ShowWarning(message)).await;
    }

    async fn on_debug(&mut self, event: DebugEvent) {
        if let DebugEvent::LlmCall {
            model,
            route,
            input_tokens,
            output_tokens,
            cache_creation_input_tokens,
            cache_read_input_tokens,
            ..
        } = event
        {
            if model.is_empty() {
                return;
            }
            let call = ModelCall {
                model,
                route,
                purpose: None,
                usage: Usage {
                    input_tokens,
                    output_tokens,
                    cache_creation_input_tokens,
                    cache_read_input_tokens,
                },
            };
            let stats = call_stats(&call, self.state.context_limit);
            self.state.usage.add(&call_session_usage(&stats));
            self.state.turn_cost_usd += stats.cost_usd.unwrap_or(0.0);
            self.ui.send(UiCommand::CallStats(stats)).await;
            self.enforce_cost_cap().await;
        }
    }

    async fn on_progress(
        &mut self,
        stage: String,
        tool_name: Option<String>,
        _elapsed_ms: Option<u64>,
        message: Option<String>,
    ) {
        match (stage.as_str(), message) {
            (PROGRESS_TODOS_UPDATED, Some(message)) => {
                match serde_json::from_str::<Vec<TodoItem>>(&message) {
                    Ok(todos) => {
                        let views = todo_views(&todos);
                        let items = todos
                            .iter()
                            .map(|todo| todo.id.clone())
                            .zip(views.iter().cloned())
                            .collect();
                        self.plan.tasks.set_items(self.plan.agent, items).await;
                        self.ui.send(UiCommand::SetTodos(views)).await;
                        self.sync_plan(&todos).await;
                    }
                    Err(e) => debug!(error = %e, "Unreadable todo list update"),
                }
            }
            (PROGRESS_CONTEXT_SUMMARIZED, Some(message)) => {
                self.ui.send(UiCommand::Compacted(message)).await;
            }
            (PROGRESS_TODOS_UPDATED | PROGRESS_CONTEXT_SUMMARIZED, None) => {}
            (_, Some(message)) => self.ui.send(UiCommand::SetStatus(message)).await,
            (_, None) => {
                let status = match tool_name {
                    Some(tool) => format!("{stage}: {tool}"),
                    None => stage,
                };
                self.ui.send(UiCommand::SetStatus(status)).await;
            }
        }
    }

    async fn on_iteration_complete(
        &mut self,
        iteration: usize,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        // A later model call of the turn may answer a nudge or a notice
        // with text and no tool call first: that is a reply of its own.
        self.finish_streaming_if_active().await;
        self.turn_tokens += input_tokens + output_tokens;
        self.ui
            .send(UiCommand::SetStatus(iteration_status(
                iteration,
                self.turn_tokens,
            )))
            .await;
    }

    // `StepComplete` and `ToolComplete` use the default no-op impls; the
    // TUI derives UI transitions from the events above.
}
