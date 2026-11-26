//! Building the [`ModelRequest`] for an iteration from [`LoopState`].

use aura_model::{Message, ModelRequest, ModelRequestKind, Role, ThinkingEffort, ToolDefinition};

use super::super::config::{parse_cache_retention, AgentLoopConfig};
use super::LoopState;

/// `effort` one step lower; the lowest steps stay where they are.
fn one_lower(effort: ThinkingEffort) -> ThinkingEffort {
    match effort {
        ThinkingEffort::Max | ThinkingEffort::XHigh => ThinkingEffort::High,
        ThinkingEffort::High => ThinkingEffort::Medium,
        ThinkingEffort::Medium => ThinkingEffort::Low,
        lowest => lowest,
    }
}

impl LoopState {
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

        let room = super::super::context::room_left_for_reply(config, self, &effective_tools);
        let builder = ModelRequest::builder(&config.model, &config.system_prompt);
        let builder = match room {
            Some(room) => builder.max_tokens(room),
            None => builder,
        };
        builder
            .messages(self.messages.shared())
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
