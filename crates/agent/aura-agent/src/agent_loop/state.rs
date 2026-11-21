//! Mutable per-run loop state and its inherent methods.
//!
//! Carved out of `agent_loop/mod.rs` during the Phase 3 god-module
//! split. The struct and its `begin_iteration` / `build_request` /
//! `compute_thinking_effort` / `latest_user_text` helpers live here;
//! the [`super::AgentLoop`] methods that drive a whole run still live
//! in [`super::run`] and [`super::stop_reason`].

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use aura_core_types::trace::HarnessDecision;
use aura_model::{Message, ModelRequest, ModelRequestKind, Role, ThinkingEffort, ToolDefinition};

use crate::budget::{BudgetState, ExplorationState};
use crate::types::{AgentLoopResult, BuildBaseline};

use super::cache::ToolResultCache;
use super::config::{parse_cache_retention, AgentLoopConfig};
use super::steering::SteeringRegistry;
use super::{steering, turn_diff};

/// `effort` one step lower; the lowest steps stay where they are.
fn one_lower(effort: ThinkingEffort) -> ThinkingEffort {
    match effort {
        ThinkingEffort::Max | ThinkingEffort::XHigh => ThinkingEffort::High,
        ThinkingEffort::High => ThinkingEffort::Medium,
        ThinkingEffort::Medium => ThinkingEffort::Low,
        lowest => lowest,
    }
}

/// Mutable state carried across iterations of the agent loop.
pub(crate) struct LoopState {
    pub(crate) result: AgentLoopResult,
    pub(crate) tool_cache: ToolResultCache,
    pub(crate) exploration_state: ExplorationState,
    pub(crate) budget_state: BudgetState,
    pub(crate) had_any_write: bool,
    /// Set true the first iteration whose tool results contain any
    /// `FileOp` (any successful `write_file` / `edit_file` /
    /// `delete_file`). Cumulative across the run — never reset.
    /// Consumed by the reasoning-effort policy to drop to `Low`
    /// effort once forward motion has happened.
    pub(crate) had_any_file_write: bool,
    /// Set true when `handle_task_done` successfully returns
    /// `stop_loop = true` (i.e. all DoD gates passed). Cumulative
    /// across the run — never reset.
    ///
    /// Wired in `tool_execution::check_termination_conditions` by
    /// observing a non-error tool result whose source tool is
    /// `task_done` and whose `stop_loop` flag is set.
    pub(crate) task_done_completed: bool,
    pub(crate) output_limit_recoveries: usize,
    /// Phase 2: set to `true` the iteration after a successful
    /// `submit_plan` accept has been observed via
    /// [`AgentLoopConfig::phase_reset_signal`]. Cumulative across the
    /// run — never reset. Drives [`Self::compute_thinking_effort`] to
    /// drop to `Low` once a plan exists, mirroring codex's
    /// post-plan `reasoning.effort=low` behaviour.
    ///
    /// We deliberately reuse the existing `phase_reset_signal`
    /// handshake (set by `handle_submit_plan` in the task executor)
    /// instead of inventing a parallel signal path. The first
    /// iteration's flip is the task-start pre-seed (see
    /// `agent_runner::execute_task_tracked`), so we only treat
    /// observations on `iteration > 0` as real submit_plan acceptances.
    pub(crate) submit_plan_called: bool,
    pub(crate) checkpoint_emitted: bool,
    pub(crate) exploration_compaction_done: bool,
    pub(crate) build_cooldown: usize,
    /// How many steps below the policy's effort the rest of the run
    /// reasons at, once replies ran out of room before they acted
    /// ([`Self::reason_less`]).
    pub(crate) effort_lowered: u8,
    pub(crate) last_context_tokens_estimate: Option<u64>,
    pub(crate) messages: Vec<Message>,
    pub(crate) build_baseline: Option<BuildBaseline>,
    /// Per-iteration net file-op accumulator. Reset at the top of
    /// every iteration. Tracks writes so the
    /// `had_any_file_write` latch lights up via
    /// `tool_pipeline::track_tool_effects` and tool-result caching
    /// invariants stay path-aware.
    pub(crate) turn_diff: turn_diff::TurnDiff,
    /// Paths successfully read this session; used by the duplicate-read gate.
    pub(crate) session_read_paths: std::collections::HashSet<PathBuf>,
    /// Per-path read budget granted after a successful write to that path.
    /// Lets the agent inspect changed regions while repairing malformed edits.
    pub(crate) read_after_write_allowances: std::collections::HashMap<PathBuf, u8>,
    /// One-shot latch mirrored from
    /// [`steering::SteeringRegistry::implement_now_injected`]. Kept on
    /// `LoopState` because the pre-dispatch
    /// `tool_pipeline::partition_circling_duplicate_reads` gate
    /// consults it on every batch and rebinding the borrow each call
    /// is needlessly noisy; the field is refreshed in
    /// [`Self::begin_iteration`] right after the registry drains.
    pub(crate) implement_now_injected: bool,
    /// Phase 5: every per-turn steering evaluator installed for this
    /// run. Sources implement [`steering::TurnSteering`] and are
    /// driven by the loop via [`Self::begin_iteration`] (which calls
    /// `begin_turn` + `drain_for_next_turn`) and by
    /// `tool_pipeline::track_tool_effects` (which calls
    /// `observe_tool` on every `(tool, result)` pair).
    pub(crate) steering: SteeringRegistry,
    /// Identical failed edit attempts, retained across intervening reads.
    pub(crate) failed_edit_attempts: std::collections::HashMap<String, u32>,
    /// Todo list as of the last open-todo nudge, and how many nudges in
    /// a row left it unchanged. See `turn::open_todo_nudge`.
    pub(crate) todos_at_last_nudge: Option<Vec<aura_exec_tools::todo::TodoItem>>,
    pub(crate) stalled_todo_nudges: u32,
    /// Harness decisions made since the last flush to the event
    /// channel. Sites without an event sender queue here and
    /// [`super::decisions::flush`] drains them.
    pub(crate) decisions: Vec<HarnessDecision>,
    /// Reasoning effort sent on the previous request, so effort
    /// changes are recorded once rather than every iteration.
    pub(crate) last_thinking_effort: Option<String>,
    /// Tool results the streaming pump already sent to the client as each
    /// call finished; the dispatch tail skips them and clears the set.
    pub(crate) reported_tool_results: std::collections::HashSet<String>,
    /// Messages before this index were looked through for the reports
    /// of detached tasks (see [`Self::observe_task_reports`]).
    task_reports_seen: usize,
}

impl LoopState {
    pub(crate) fn decide(&mut self, decision: HarnessDecision) {
        self.decisions.push(decision);
    }

    pub(super) fn new(config: &AgentLoopConfig, messages: Vec<Message>) -> Self {
        let task_reports_seen = messages
            .iter()
            .rposition(|m| m.role == Role::Assistant)
            .map_or(0, |at| at + 1);
        let checkpoint_emitted = messages.iter().any(|m| {
            m.role == Role::User
                && m.text_content()
                    .contains(super::context::FIRST_WRITE_CHECKPOINT)
        });
        Self {
            result: AgentLoopResult {
                todos: config.initial_todos.clone(),
                ..AgentLoopResult::default()
            },
            tool_cache: ToolResultCache::default(),
            exploration_state: ExplorationState::default(),
            budget_state: BudgetState::default(),
            had_any_write: false,
            had_any_file_write: false,
            task_done_completed: false,
            output_limit_recoveries: 0,
            submit_plan_called: false,
            checkpoint_emitted,
            exploration_compaction_done: false,
            build_cooldown: 0,
            effort_lowered: 0,
            last_context_tokens_estimate: None,
            messages,
            build_baseline: None,
            turn_diff: turn_diff::TurnDiff::default(),
            session_read_paths: std::collections::HashSet::new(),
            read_after_write_allowances: std::collections::HashMap::new(),
            implement_now_injected: false,
            steering: SteeringRegistry::for_loop(
                config.phase_reset_signal.is_some(),
                config.early_test_oracle.clone(),
            ),
            failed_edit_attempts: std::collections::HashMap::new(),
            decisions: Vec::new(),
            last_thinking_effort: None,
            todos_at_last_nudge: None,
            stalled_todo_nudges: 0,
            reported_tool_results: std::collections::HashSet::new(),
            task_reports_seen,
        }
    }

    /// Apply every `todo_write` call in `response` to the tracked list,
    /// give the todo of every `task` call to its agent, and tell the
    /// client about the new list.
    pub(super) fn observe_todo_writes(
        &mut self,
        response: &aura_model::ModelResponse,
        event_tx: Option<&tokio::sync::mpsc::Sender<crate::events::AgentLoopEvent>>,
    ) {
        use aura_exec_tools::agents::task::TASK_TOOL_NAME;
        use aura_exec_tools::todo::{apply_todo_write, assign_task, TODO_WRITE_TOOL_NAME};
        let mut changed = self.observe_task_reports();
        for block in &response.message.content {
            let aura_model::ContentBlock::ToolUse { name, input, .. } = block else {
                continue;
            };
            if name == TASK_TOOL_NAME {
                if let Some(next) = assign_task(&self.result.todos, input) {
                    self.result.todos = next;
                    changed = true;
                }
                continue;
            }
            if name != TODO_WRITE_TOOL_NAME {
                continue;
            }
            match apply_todo_write(&self.result.todos, input) {
                Ok(next) => {
                    self.result.todos = next;
                    changed = true;
                }
                Err(err) => tracing::debug!(%err, "ignoring malformed todo_write call"),
            }
        }
        if changed {
            self.emit_todos(event_tx);
        }
    }

    /// Close the todos of the `task` calls in `calls` that reported back
    /// in `results`: done when the agent finished, pending again when it
    /// failed. Detached tasks report later, in a notice.
    pub(super) fn observe_task_results(
        &mut self,
        calls: &[crate::types::ToolCallInfo],
        results: &[crate::types::ToolCallResult],
        event_tx: Option<&tokio::sync::mpsc::Sender<crate::events::AgentLoopEvent>>,
    ) {
        use aura_exec_tools::agents::task::TASK_TOOL_NAME;
        use aura_exec_tools::todo::{report_task, task_todo};
        let mut changed = false;
        for result in results {
            let Some(call) = calls
                .iter()
                .find(|call| call.id == result.tool_use_id && call.name == TASK_TOOL_NAME)
            else {
                continue;
            };
            let Some((id, true)) = task_todo(&call.input) else {
                continue;
            };
            if let Some(next) = report_task(&self.result.todos, id, !result.is_error) {
                self.result.todos = next;
                changed = true;
            }
        }
        if changed {
            self.emit_todos(event_tx);
        }
    }

    /// Close the todos of detached tasks whose reports (see
    /// [`aura_exec_tools::todo::task_report_line`]) came in since the
    /// last look, as a prompt or a message steered into the turn.
    /// Returns whether any todo changed.
    fn observe_task_reports(&mut self) -> bool {
        use aura_exec_tools::todo::{report_task, task_reports};
        let start = self.task_reports_seen.min(self.messages.len());
        self.task_reports_seen = self.messages.len();
        let mut changed = false;
        for message in self.messages[start..]
            .iter()
            .filter(|m| m.role == Role::User)
        {
            for block in &message.content {
                let aura_model::ContentBlock::Text { text } = block else {
                    continue;
                };
                for (id, ok) in task_reports(text) {
                    if let Some(next) = report_task(&self.result.todos, &id, ok) {
                        self.result.todos = next;
                        changed = true;
                    }
                }
            }
        }
        changed
    }

    fn emit_todos(
        &self,
        event_tx: Option<&tokio::sync::mpsc::Sender<crate::events::AgentLoopEvent>>,
    ) {
        use aura_exec_tools::todo::TODO_WRITE_TOOL_NAME;
        if let Ok(json) = serde_json::to_string(&self.result.todos) {
            super::streaming::emit(
                event_tx,
                crate::events::AgentLoopEvent::Progress {
                    stage: crate::events::PROGRESS_TODOS_UPDATED.to_string(),
                    tool_name: Some(TODO_WRITE_TOOL_NAME.to_string()),
                    elapsed_ms: None,
                    message: Some(json),
                },
            );
        }
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn new_for_tests(config: &AgentLoopConfig, messages: Vec<Message>) -> Self {
        Self::new(config, messages)
    }

    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    pub(crate) fn begin_iteration(&mut self, config: &AgentLoopConfig, iteration: usize) {
        self.build_cooldown = self.build_cooldown.saturating_sub(1);

        // Phase 1.A: scope the turn-diff to the iteration we are about
        // to execute. The previous iteration's net file ops are no
        // longer relevant — `had_any_file_write` (cumulative latch)
        // and the per-iteration `turn_diff` answer different questions.
        self.turn_diff.reset();

        // Phase 5: drive every installed `TurnSteering` source
        // through one uniform begin-turn → drain pipeline. The
        // registry calls `begin_turn` on every source, then
        // returns the concatenated `drain_for_next_turn` output
        // which the loop routes through the existing `inject`
        // helper. The pre-Phase-5 inline `begin_turn` /
        // `evaluate_implement_now` calls are gone.
        self.steering.begin_turn();
        for kind in self.steering.drain_for_next_turn() {
            steering::inject(&mut self.messages, &kind);
            let decision = super::decisions::steering_nudge(&kind, self, iteration);
            self.decide(decision);
        }
        // Mirror the registry's `implement_now_injected` latch back
        // onto `LoopState` so the pre-dispatch circling-read gate in
        // `tool_pipeline::partition_circling_duplicate_reads`
        // continues to be a synchronous read on `state` rather than
        // taking a fresh borrow on `state.steering` every batch.
        self.implement_now_injected = self.steering.implement_now_injected();

        // Observe-and-clear the optional handshake from a wrapping
        // `TaskToolExecutor`: when `submit_plan` is accepted the
        // executor flips this shared `Arc<AtomicBool>` to `true`, and
        // the loop must zero out the exploration counter so the
        // implement phase has a fresh budget instead of inheriting the
        // exploration phase's exhausted one.
        // `exploration_compaction_done` is cleared so proactive
        // compaction can fire once more during the implement phase.
        if let Some(ref signal) = config.phase_reset_signal {
            if signal.swap(false, Ordering::AcqRel) {
                tracing::info!(
                    old_exploration_count = self.exploration_state.count,
                    "submit_plan accepted: resetting exploration counter"
                );
                self.exploration_state.count = 0;
                self.exploration_compaction_done = false;
                // Phase 2: latch the "submit_plan was accepted" signal
                // for the effort policy. The reset signal is also flipped
                // at task start (pre-seeded `true` in
                // `agent_runner::execute_task_tracked` so the first
                // iteration's reset path fires), so we only treat
                // observations on `iteration > 0` as real submit_plan
                // acceptances. Iteration 0's flip is the task-start
                // pre-seed and must not toggle the effort policy.
                if iteration > 0 {
                    self.submit_plan_called = true;
                }
            }
        }
    }

    /// Reason one step lower for the rest of the run, after a reply spent
    /// the model's whole output limit before it made a tool call: one at
    /// the same effort spends it the same way.
    pub(crate) fn reason_less(&mut self) {
        self.effort_lowered = self.effort_lowered.saturating_add(1);
    }

    /// Reasoning-effort policy applied per iteration.
    /// Codex sets `reasoning.effort` explicitly per Responses API call
    /// (codex-rs/core/src/client.rs:698-714); the rules below are the
    /// aura analog tailored to aura's `write_file`/`edit_file`/
    /// `delete_file` surface.
    ///
    /// **Temporary (2026-05): dev-loop turns are pinned to
    /// [`ThinkingEffort::Medium`] regardless of iteration / write /
    /// plan state.** We're evaluating whether holding a single effort
    /// level across the run converges faster than the codex-style
    /// `Off → Medium → Low` taper. `disable_thinking_iteration_0` is
    /// only set by `configure_loop_config` for dev-loop tasks, so chat
    /// and other callers retain the original tiered policy below.
    ///
    /// A model that reasons high
    /// ([`aura_model::model_catalog::reasons_high`]: Claude Sonnet 5.5
    /// and Opus 5) gets `High` on every iteration of every run, dev-loop
    /// runs included. The effort never changes within a run, so its
    /// prompt cache is kept.
    ///
    /// Resolution order for other models and non-dev-loop callers (first
    /// match wins):
    ///
    /// 1. Iteration 0 → `Medium` (analysis turn).
    /// 2. A model whose prompt cache is reset by a change of effort
    ///    (Claude) → `Medium` for the whole run. Dropping to `Low` after
    ///    the first write made the next request write the whole
    ///    conversation to the cache again, which costs more than the
    ///    lower effort saves.
    /// 3. `had_any_file_write` → `Low` (forward motion has happened,
    ///    cap the deliberation budget).
    /// 4. `submit_plan_called` → `Low` (the plan exists; codex drops
    ///    to low effort once the agent is committed to an
    ///    implementation phase).
    /// 5. Otherwise → `Medium`.
    pub(crate) fn compute_thinking_effort(
        &self,
        config: &AgentLoopConfig,
        iteration: usize,
    ) -> ThinkingEffort {
        if aura_model::model_catalog::reasons_high(&config.model) {
            return ThinkingEffort::High;
        }
        if config.disable_thinking_iteration_0 {
            return ThinkingEffort::Medium;
        }
        if iteration == 0 {
            return ThinkingEffort::Medium;
        }
        if aura_model::model_catalog::effort_change_resets_prompt_cache(&config.model) {
            return ThinkingEffort::Medium;
        }
        if self.had_any_file_write || self.submit_plan_called {
            return ThinkingEffort::Low;
        }
        ThinkingEffort::Medium
    }

    pub(crate) fn build_request(
        &self,
        config: &AgentLoopConfig,
        tools: &[ToolDefinition],
        iteration: usize,
    ) -> Result<ModelRequest, crate::AgentError> {
        let effective_tools = scope_tools_for_iteration(config, &self.messages, tools);
        let request_kind = pick_request_kind(&effective_tools, config.request_kind, iteration);

        // The cook-loop-fix strip (2026-05) removed the read-only
        // streak counter and the force-tool-choice path that rode on
        // top of it. `tool_choice` is always `Auto`; the model picks
        // its own next move.
        let tool_choice = aura_model::ToolChoice::Auto;

        // Codex parity: emit an explicit `reasoning.effort` on every
        // request. The reasoner's `max_tokens > 2048` auto-enable
        // path stays as a fallback for providers that ignore the
        // explicit field.
        //
        // A user-selected thinking level (forwarded from the chat model
        // picker via `AgentLoopConfig::user_thinking_effort`) hard-pins
        // the effort across every iteration, overriding the internal
        // `compute_thinking_effort` taper so the operator's explicit
        // choice always wins. Then the effort the router chose for the
        // work; when neither is set, fall back to the heuristic. Either is
        // lowered once replies ran out of room ([`Self::reason_less`]).
        let thinking_effort = Some(config.user_thinking_effort.unwrap_or_else(|| {
            let policy = config
                .routed_thinking_effort
                .unwrap_or_else(|| self.compute_thinking_effort(config, iteration));
            (0..self.effort_lowered).fold(policy, |effort, _| one_lower(effort))
        }));

        let room = super::context::room_left_for_reply(config, self, &effective_tools);
        let builder = ModelRequest::builder(&config.model, &config.system_prompt);
        let builder = match room {
            Some(room) => builder.max_tokens(room),
            None => builder,
        };
        builder
            .messages(self.messages.clone())
            .tools(effective_tools)
            .tool_choice(tool_choice)
            .thinking_effort(thinking_effort)
            .auth_token(config.auth_token.clone())
            .upstream_provider_family(config.upstream_provider_family.clone())
            .aura_project_id(config.aura_project_id.clone())
            .aura_agent_id(config.aura_agent_id.clone())
            .aura_session_id(config.aura_session_id.clone())
            .aura_org_id(config.aura_org_id.clone())
            .prompt_cache_key(config.prompt_cache_key.clone())
            .prompt_cache_retention(parse_cache_retention(
                config.prompt_cache_retention.as_deref(),
            ))
            .request_kind(request_kind)
            .try_build()
            .map_err(crate::AgentError::from)
    }
}

/// Narrow `tools` down to domain-relevant entries before the
/// tool-hints logic runs. The classifier is keyed on the most recent
/// pure-text user message, so scratchpad tool-result turns reuse the
/// previous filter rather than widening the surface back to every tool.
fn scope_tools_for_iteration(
    config: &AgentLoopConfig,
    messages: &[Message],
    tools: &[ToolDefinition],
) -> Vec<ToolDefinition> {
    let classifier_filtered: Vec<ToolDefinition> = match (
        config.intent_classifier.as_deref(),
        latest_user_text(messages),
    ) {
        (Some(classifier), Some(text)) if !config.intent_classifier_manifest.is_empty() => {
            classifier.filter_tools(text, &config.intent_classifier_manifest, tools)
        }
        _ => tools.to_vec(),
    };

    match &config.tool_hints {
        Some(hints) if !hints.is_empty() => {
            let filtered: Vec<_> = classifier_filtered
                .iter()
                .filter(|t| hints.iter().any(|h| h == &t.name))
                .cloned()
                .collect();
            if filtered.is_empty() {
                classifier_filtered
            } else {
                filtered
            }
        }
        _ => classifier_filtered,
    }
}

/// Resolve [`ModelRequestKind`] for the current build_request call.
///
/// Narrow the project-tool override to dev-loop turns only.
///
/// The `ProjectToolTaskExtract` / `ProjectToolSpecGen` request kinds
/// carry a `PROJECT_TOOL_TOTAL_TEXT_MAX_BYTES = 48 KiB` cap in
/// `aura-reasoner::content_profile`. The cap exists so the
/// task-extraction phase of the dev loop can't blow up the model
/// request with arbitrary chat history. The previous wildcard
/// arm — `(true, _, _, _) => ProjectToolTaskExtract` — clobbered
/// any explicit `config.request_kind` (including `Chat`) whenever
/// the task tools happened to be visible. That makes every chat
/// turn for an agent with `create_task`/etc. in scope hard-fail
/// with `EmergencyCapRequired` once history accumulates past
/// ~48 KiB, even though normal chat conversations should be
/// governed by the much-larger chat budget instead.
///
/// Restrict the override to `DevLoopBootstrap`/`Continuation`
/// request kinds, where the task-extraction context invariant
/// actually applies. Plain `Chat` / `Auxiliary` requests now keep
/// their declared `config.request_kind` even when they happen to
/// have task / spec tools available.
fn pick_request_kind(
    effective_tools: &[ToolDefinition],
    request_kind: ModelRequestKind,
    iteration: usize,
) -> ModelRequestKind {
    let has_task_tools = effective_tools.iter().any(|tool| {
        matches!(
            tool.name.as_str(),
            "create_task" | "update_task" | "list_tasks" | "get_task" | "delete_task"
        )
    });
    let has_spec_tools = effective_tools.iter().any(|tool| {
        matches!(
            tool.name.as_str(),
            "create_spec" | "update_spec" | "list_specs" | "get_spec" | "delete_spec"
        )
    });
    match (has_task_tools, has_spec_tools, request_kind, iteration) {
        (
            true,
            _,
            ModelRequestKind::DevLoopBootstrap | ModelRequestKind::DevLoopContinuation,
            _,
        ) => ModelRequestKind::ProjectToolTaskExtract,
        (
            _,
            true,
            ModelRequestKind::DevLoopBootstrap | ModelRequestKind::DevLoopContinuation,
            _,
        ) => ModelRequestKind::ProjectToolSpecGen,
        (_, _, ModelRequestKind::DevLoopBootstrap, 0) => ModelRequestKind::DevLoopBootstrap,
        (_, _, ModelRequestKind::DevLoopBootstrap, _) => ModelRequestKind::DevLoopContinuation,
        (_, _, kind, _) => kind,
    }
}

/// Return the text of the most recent user-role message whose content is
/// plain text (skipping tool-result turns, which carry tool output rather
/// than a natural-language intent).
///
/// Used by [`LoopState::build_request`] to feed the intent classifier on
/// every iteration — including scratchpad iterations that follow a tool
/// call — so the tool filter stays keyed on the original user intent
/// until the user speaks again.
pub(super) fn latest_user_text(messages: &[Message]) -> Option<&str> {
    for msg in messages.iter().rev() {
        if matches!(msg.role, Role::User)
            && msg
                .content
                .iter()
                .any(|b| matches!(b, aura_model::ContentBlock::Text { .. }))
        {
            return msg.content.iter().find_map(|b| match b {
                aura_model::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            });
        }
    }
    None
}

#[cfg(test)]
mod intent_classifier_tests {
    use super::*;
    use aura_exec_tools::IntentClassifier;
    use aura_model::ToolDefinition;
    use serde_json::json;
    use std::sync::Arc;

    fn mk_tool(name: &str) -> ToolDefinition {
        ToolDefinition::new(name, name, json!({}))
    }

    fn mk_config_with_classifier() -> AgentLoopConfig {
        let classifier = IntentClassifier::from_rules(
            vec!["project".to_string()],
            vec![("billing".to_string(), vec!["credit".to_string()])],
        );
        AgentLoopConfig {
            intent_classifier: Some(Arc::new(classifier)),
            intent_classifier_manifest: vec![
                ("create_project".to_string(), "project".to_string()),
                ("list_credits".to_string(), "billing".to_string()),
            ],
            ..AgentLoopConfig::for_agent("claude-test-model")
        }
    }

    #[test]
    fn build_request_filters_tier2_tools_when_not_triggered() {
        let config = mk_config_with_classifier();
        let state = LoopState::new(&config, vec![Message::user("hello there")]);
        let tools = vec![
            mk_tool("create_project"),
            mk_tool("list_credits"),
            mk_tool("read_file"),
        ];

        let req = state.build_request(&config, &tools, 1).unwrap();
        let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();

        assert!(names.contains(&"create_project"), "tier-1 tool kept");
        assert!(names.contains(&"read_file"), "unmapped tool passes through");
        assert!(
            !names.contains(&"list_credits"),
            "tier-2 billing tool hidden"
        );
    }

    #[test]
    fn build_request_admits_tier2_when_keyword_matches() {
        let config = mk_config_with_classifier();
        let state = LoopState::new(
            &config,
            vec![Message::user("check my credit balance please")],
        );
        let tools = vec![mk_tool("create_project"), mk_tool("list_credits")];

        let req = state.build_request(&config, &tools, 1).unwrap();
        let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"list_credits"));
        assert!(names.contains(&"create_project"));
    }

    #[test]
    fn build_request_skips_tool_result_messages_when_picking_intent() {
        let config = mk_config_with_classifier();
        let msgs = vec![
            Message::user("check my credit balance"),
            Message::assistant("calling tool"),
            Message::tool_results(vec![(
                "tu_1".into(),
                aura_model::ToolResultContent::Text("100".into()),
                false,
            )]),
        ];
        let state = LoopState::new(&config, msgs);
        let tools = vec![mk_tool("list_credits"), mk_tool("create_project")];

        let req = state.build_request(&config, &tools, 2).unwrap();
        let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"list_credits"),
            "classifier should still see original user message after a tool-result turn"
        );
    }

    #[test]
    fn build_request_passthrough_when_classifier_absent() {
        let config = AgentLoopConfig::for_agent("claude-test-model");
        let state = LoopState::new(&config, vec![Message::user("anything")]);
        let tools = vec![mk_tool("anything_tool")];
        let req = state.build_request(&config, &tools, 1).unwrap();
        assert_eq!(req.tools.len(), 1);
    }

    #[test]
    fn build_request_keeps_tool_hints_scoped_after_first_iteration() {
        let config = AgentLoopConfig {
            tool_hints: Some(vec!["read_file".to_string(), "create_task".to_string()]),
            ..AgentLoopConfig::for_agent("claude-test-model")
        };
        let msgs = vec![
            Message::user("extract tasks"),
            Message::assistant("calling tool"),
            Message::tool_results(vec![(
                "tu_1".into(),
                aura_model::ToolResultContent::Text("large requirements".into()),
                false,
            )]),
        ];
        let state = LoopState::new(&config, msgs);
        let tools = vec![
            mk_tool("read_file"),
            mk_tool("create_task"),
            mk_tool("run_command"),
            mk_tool("generate_image"),
        ];

        let req = state.build_request(&config, &tools, 2).unwrap();
        let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();

        assert_eq!(names, vec!["read_file", "create_task"]);
        assert!(matches!(req.tool_choice, aura_model::ToolChoice::Auto));
    }

    #[test]
    fn build_request_keeps_tool_hints_auto_on_first_iteration() {
        let config = AgentLoopConfig {
            tool_hints: Some(vec!["read_file".to_string(), "create_task".to_string()]),
            ..AgentLoopConfig::for_agent("claude-test-model")
        };
        let state = LoopState::new(&config, vec![Message::user("extract tasks")]);
        let tools = vec![
            mk_tool("read_file"),
            mk_tool("create_task"),
            mk_tool("run_command"),
        ];

        let req = state.build_request(&config, &tools, 0).unwrap();
        let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();

        assert_eq!(names, vec!["read_file", "create_task"]);
        assert!(matches!(req.tool_choice, aura_model::ToolChoice::Auto));
    }

    /// Regression: a `Chat` request with `create_task` in scope must
    /// keep `request_kind = Chat`, NOT silently get re-classified as
    /// `ProjectToolTaskExtract`. The latter carries a 48 KiB total-text
    /// budget in `aura-reasoner::content_profile`, so the old
    /// reclassification turned every chat for an agent-with-task-tools
    /// into a hard `EmergencyCapRequired` failure once history grew
    /// past ~48 KiB. The fix narrows the override to dev-loop turns.
    #[test]
    fn build_request_keeps_chat_kind_when_task_tools_visible() {
        let config = AgentLoopConfig {
            request_kind: ModelRequestKind::Chat,
            ..AgentLoopConfig::for_agent("claude-test-model")
        };
        let state = LoopState::new(&config, vec![Message::user("hi there")]);
        let tools = vec![mk_tool("create_task"), mk_tool("read_file")];

        let req = state.build_request(&config, &tools, 0).unwrap();
        assert_eq!(
            req.metadata.kind,
            Some(ModelRequestKind::Chat),
            "Chat must stay Chat even when task tools are visible (otherwise EmergencyCapRequired blocks chat at 48 KiB)"
        );
    }

    /// Companion: same invariant for spec tools — `create_spec` etc.
    /// in scope must not flip a `Chat` turn into `ProjectToolSpecGen`.
    #[test]
    fn build_request_keeps_chat_kind_when_spec_tools_visible() {
        let config = AgentLoopConfig {
            request_kind: ModelRequestKind::Chat,
            ..AgentLoopConfig::for_agent("claude-test-model")
        };
        let state = LoopState::new(&config, vec![Message::user("hi")]);
        let tools = vec![mk_tool("create_spec"), mk_tool("read_file")];

        let req = state.build_request(&config, &tools, 0).unwrap();
        assert_eq!(req.metadata.kind, Some(ModelRequestKind::Chat));
    }

    /// The dev-loop flow IS still subject to the project-tool override:
    /// when the caller declares `DevLoopBootstrap` AND task tools are
    /// available, the iteration after iteration `0` must report
    /// `ProjectToolTaskExtract` (the existing extraction-phase guard).
    /// Pins the narrowing didn't accidentally break the dev loop.
    #[test]
    fn build_request_promotes_devloop_to_project_tool_task_extract_when_task_tools_visible() {
        let config = AgentLoopConfig {
            request_kind: ModelRequestKind::DevLoopBootstrap,
            ..AgentLoopConfig::for_agent("claude-test-model")
        };
        let state = LoopState::new(&config, vec![Message::user("extract tasks")]);
        let tools = vec![mk_tool("create_task")];

        let req = state.build_request(&config, &tools, 1).unwrap();
        assert_eq!(
            req.metadata.kind,
            Some(ModelRequestKind::ProjectToolTaskExtract)
        );
    }

    /// Mirror for the spec branch.
    #[test]
    fn build_request_promotes_devloop_to_project_tool_spec_gen_when_spec_tools_visible() {
        let config = AgentLoopConfig {
            request_kind: ModelRequestKind::DevLoopBootstrap,
            ..AgentLoopConfig::for_agent("claude-test-model")
        };
        let state = LoopState::new(&config, vec![Message::user("extract specs")]);
        let tools = vec![mk_tool("create_spec")];

        let req = state.build_request(&config, &tools, 1).unwrap();
        assert_eq!(
            req.metadata.kind,
            Some(ModelRequestKind::ProjectToolSpecGen)
        );
    }
}

#[cfg(test)]
mod reply_size_tests {
    use super::*;

    fn next_request(state: &mut LoopState, config: &AgentLoopConfig) -> ModelRequest {
        state.begin_iteration(config, 1);
        state.build_request(config, &[], 1).unwrap()
    }

    #[test]
    fn replies_get_the_models_whole_output_limit_and_reason_less_after_a_cut_off() {
        let config = AgentLoopConfig::for_agent("claude/claude-opus-5-5");
        let mut state = LoopState::new(&config, vec![Message::user("write the module")]);
        let first = next_request(&mut state, &config);
        assert_eq!(first.max_tokens.get(), 128_000);
        assert_eq!(first.thinking_effort, Some(ThinkingEffort::High));

        state.reason_less();
        let second = next_request(&mut state, &config);
        assert_eq!(second.max_tokens.get(), 128_000);
        assert_eq!(second.thinking_effort, Some(ThinkingEffort::Medium));

        state.reason_less();
        let later = next_request(&mut state, &config);
        assert_eq!(later.thinking_effort, Some(ThinkingEffort::Low));
        assert_eq!(
            next_request(&mut state, &config).thinking_effort,
            Some(ThinkingEffort::Low)
        );
    }

    #[test]
    fn a_routed_effort_is_lowered_after_a_cut_off_too() {
        let config = AgentLoopConfig {
            routed_thinking_effort: Some(ThinkingEffort::Max),
            ..AgentLoopConfig::for_agent("claude/claude-opus-5-5")
        };
        let mut state = LoopState::new(&config, vec![Message::user("write the module")]);
        assert_eq!(
            next_request(&mut state, &config).thinking_effort,
            Some(ThinkingEffort::Max)
        );
        state.reason_less();
        assert_eq!(
            next_request(&mut state, &config).thinking_effort,
            Some(ThinkingEffort::High)
        );
    }

    #[test]
    fn a_chosen_effort_stands() {
        let config = AgentLoopConfig {
            user_thinking_effort: Some(ThinkingEffort::Max),
            ..AgentLoopConfig::for_agent("claude/claude-opus-5-5")
        };
        let mut state = LoopState::new(&config, vec![Message::user("write the module")]);

        state.reason_less();
        let request = next_request(&mut state, &config);
        assert_eq!(request.thinking_effort, Some(ThinkingEffort::Max));
    }
}
