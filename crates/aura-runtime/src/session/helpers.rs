//! Helper functions for WebSocket session management: init, executor
//! construction, event forwarding, and turn finalization.

use super::ws_handler::populate_tool_definitions;
use super::{Session, WsContext};
use crate::executor_factory;
use crate::protocol::{
    tool_info_from_definition_with_state, AssistantMessageEnd, ErrorMsg, FilesChanged,
    OutboundMessage, SessionInit, SessionReady, SessionUsage, SkillInfo, TextDelta, ThinkingDelta,
    ToolCallSnapshot, ToolInfo, ToolResultMsg, ToolUseStart,
};
use crate::runtime_capabilities;
use async_trait::async_trait;
use aura_agent::{
    map_agent_loop_event, AgentLoopEvent, AgentLoopResult, DebugEvent, TurnEventSink,
};
use aura_core::{resolve_effective_permission, ToolState, UserToolDefaults};
use aura_kernel::{Kernel, KernelConfig, PolicyConfig};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info};

fn summarize_files_changed(loop_result: &AgentLoopResult) -> FilesChanged {
    let mut files_changed = FilesChanged::default();
    for change in &loop_result.file_changes {
        match change.kind {
            aura_agent::FileChangeKind::Create => files_changed.created.push(change.path.clone()),
            aura_agent::FileChangeKind::Modify => files_changed.modified.push(change.path.clone()),
            aura_agent::FileChangeKind::Delete => files_changed.deleted.push(change.path.clone()),
        }
    }
    files_changed
}

fn resolve_session_workspace(session: &Session) -> (std::path::PathBuf, bool) {
    if let Some(ref project_path) = session.project_path {
        return (project_path.clone(), true);
    }

    if session.workspace != session.workspace_base {
        return (session.workspace.clone(), true);
    }

    (session.workspace.clone(), false)
}

fn session_user_defaults(
    session: &Session,
    ctx: &WsContext,
) -> Result<UserToolDefaults, aura_kernel::KernelError> {
    ctx.store
        .get_user_tool_defaults(&session.user_id)
        .map_err(|e| aura_kernel::KernelError::Store(format!("get_user_tool_defaults: {e}")))
        .map(|defaults| defaults.unwrap_or_default())
}

fn effective_tool_infos(session: &Session, defaults: &UserToolDefaults) -> Vec<ToolInfo> {
    session
        .tool_definitions
        .iter()
        .filter_map(|tool| {
            let state = resolve_effective_permission(
                defaults,
                session.tool_permissions.as_ref(),
                &tool.name,
            );
            (state != ToolState::Deny).then(|| tool_info_from_definition_with_state(tool, state))
        })
        .collect()
}

pub(super) async fn handle_session_init(
    session: &mut Session,
    init: SessionInit,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    ctx: &WsContext,
) {
    let provider_config = init.provider_config.clone();

    if session.initialized {
        let _ = outbound_tx.try_send(OutboundMessage::Error(ErrorMsg {
            code: "already_initialized".into(),
            message: "Session has already been initialized".into(),
            recoverable: true,
        }));
        return;
    }

    let resolved_provider_override = if let Some(provider_config) = provider_config {
        let reasoner_cfg = aura_reasoner::ProviderConfig {
            provider: provider_config.provider.clone(),
            routing_mode: provider_config.routing_mode.clone(),
            api_key: provider_config.api_key.clone(),
            base_url: provider_config.base_url.clone(),
            default_model: provider_config.default_model.clone(),
            fallback_model: provider_config.fallback_model.clone(),
            prompt_caching_enabled: provider_config.prompt_caching_enabled,
        };
        match aura_reasoner::provider_from_session_config(&reasoner_cfg) {
            Ok(selection) => Some(selection.provider),
            Err(e) => {
                let _ = outbound_tx.try_send(OutboundMessage::Error(ErrorMsg {
                    code: "invalid_provider_config".into(),
                    message: e.to_string(),
                    recoverable: true,
                }));
                return;
            }
        }
    } else {
        None
    };

    if let Err(e) = session.apply_init(init) {
        let _ = outbound_tx.try_send(OutboundMessage::Error(ErrorMsg {
            code: "invalid_workspace".into(),
            message: e,
            recoverable: true,
        }));
        return;
    }

    if session.tool_permissions.is_none() {
        match crate::tool_permissions::load_agent_tool_context(&ctx.store, session.agent_id) {
            Ok(agent_ctx) => {
                session.tool_permissions = agent_ctx.tool_permissions;
            }
            Err(e) => {
                let _ = outbound_tx.try_send(OutboundMessage::Error(ErrorMsg {
                    code: "tool_permissions_load_failed".into(),
                    message: e,
                    recoverable: true,
                }));
                return;
            }
        }
    }

    if let Some(provider) = resolved_provider_override {
        session.provider_name = provider.name().to_string();
        session.provider_override = Some(provider);
    }

    if let (Some(ref base), Some(ref pp)) = (&ctx.project_base, &session.project_path) {
        let slug = pp.file_name().and_then(|n| n.to_str()).unwrap_or("default");
        session.project_path = Some(base.join(slug));
    }

    populate_tool_definitions(session, ctx);

    match build_kernel_with_config(session, ctx, &ctx.tool_config).await {
        Ok(kernel) => {
            // Invariant §2 (Every State Change Is a Transaction) +
            // §11 (Session-Scoped Approvals): the session start is itself
            // a state change that must be recorded and that resets the
            // kernel's session-scoped approval cache. Emit the transaction
            // before anything else on this kernel so the record reflects
            // session boundaries for replay.
            if let Err(e) = kernel
                .process_direct(aura_core::Transaction::session_start(session.agent_id))
                .await
            {
                error!(
                    session_id = %session.session_id,
                    error = %e,
                    "Failed to record SessionStart transaction through kernel"
                );
            }

            if let Err(e) = runtime_capabilities::record_runtime_capabilities(
                &kernel,
                "session",
                Some(&session.session_id),
                &session.installed_tools,
                &session.installed_integrations,
            )
            .await
            {
                error!(
                    session_id = %session.session_id,
                    error = %e,
                    "Failed to record runtime capability install through kernel during session init"
                );
            }
        }
        Err(e) => {
            error!(
                session_id = %session.session_id,
                error = %e,
                "Failed to build kernel for session capability recording"
            );
        }
    }

    let defaults = match session_user_defaults(session, ctx) {
        Ok(defaults) => defaults,
        Err(e) => {
            error!(
                session_id = %session.session_id,
                error = %e,
                "Failed to load user tool defaults for SessionReady"
            );
            UserToolDefaults::default()
        }
    };
    let tools: Vec<ToolInfo> = effective_tool_infos(session, &defaults);

    let skills: Vec<SkillInfo> = match (&ctx.skill_manager, &session.skill_agent_id) {
        (Some(sm), Some(agent_id)) => {
            if let Ok(mgr) = sm.read() {
                mgr.agent_skill_meta(agent_id)
                    .into_iter()
                    .map(|m| SkillInfo {
                        name: m.name,
                        description: m.description,
                    })
                    .collect()
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    };

    info!(
        session_id = %session.session_id,
        model = %session.model,
        tool_count = tools.len(),
        integration_count = session.installed_integrations.len(),
        skill_count = skills.len(),
        "Session initialized"
    );

    let _ = outbound_tx.try_send(OutboundMessage::SessionReady(SessionReady {
        session_id: session.session_id.clone(),
        tools,
        skills,
    }));
}

pub(super) async fn build_kernel_with_config(
    session: &Session,
    ctx: &WsContext,
    tool_config: &aura_tools::ToolConfig,
) -> Result<Arc<Kernel>, aura_kernel::KernelError> {
    let domain_exec = ctx.domain_api.as_ref().map(|api| {
        use aura_tools::domain_tools::DomainToolExecutor;
        Arc::new(DomainToolExecutor::with_session_context(
            api.clone(),
            session.auth_token.clone(),
            session.project_id.clone(),
            session
                .project_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
        ))
    });

    let mut resolver =
        executor_factory::build_tool_resolver(&ctx.catalog, tool_config, domain_exec.clone())
            .with_installed_tools(session.installed_tools.clone());

    if let Some(ref controller) = ctx.automaton_controller {
        let project_id = session.project_id.clone().unwrap_or_default();
        let workspace_root = session.project_path.clone();
        for tool in aura_tools::automaton_tools::devloop_control_tools(
            controller.clone(),
            project_id,
            workspace_root,
            session.auth_token.clone(),
        ) {
            resolver.register(tool);
        }
    }

    let (workspace, use_workspace_base_as_root) = resolve_session_workspace(session);

    let mut policy = PolicyConfig::default();
    policy.set_installed_integrations(session.installed_integrations.iter().cloned());
    policy.set_tool_integration_requirements(session.installed_tools.iter().filter_map(|tool| {
        tool.required_integration
            .clone()
            .map(|requirement| (tool.name.clone(), requirement))
    }));
    // Permissions are mandatory on every session; wire them into the
    // kernel policy unconditionally so the Delegate gate enforces them.
    policy.agent_permissions = session.agent_permissions.clone();
    let user_default = session_user_defaults(session, ctx)?;
    policy = policy
        .with_user_default(user_default.clone())
        .with_agent_override(session.tool_permissions.clone());

    resolver = resolver
        .with_spawn_hook(Arc::new(aura_kernel::KernelSpawnHook::new(
            ctx.store.clone(),
        )))
        .with_caller_permissions(session.agent_permissions.clone())
        .with_tool_permission_context(user_default, session.tool_permissions.clone())
        .with_originating_user_id(session.user_id.clone());

    let router = executor_factory::build_executor_router(resolver);

    let config = KernelConfig {
        workspace_base: workspace,
        use_workspace_base_as_root,
        policy,
        tool_approval_prompter: session
            .tool_approval_broker
            .clone()
            .map(|broker| broker as Arc<dyn aura_kernel::ToolApprovalPrompter>),
        originating_user_id: Some(session.user_id.clone()),
        ..KernelConfig::default()
    };

    let kernel = Kernel::new(
        ctx.store.clone(),
        session
            .provider_override
            .clone()
            .unwrap_or_else(|| ctx.provider.clone()),
        router,
        config,
        session.agent_id,
    )?;

    Ok(Arc::new(kernel))
}

/// [`TurnEventSink`] that maps events onto the WebSocket wire protocol.
///
/// Phase 3 consolidated the handwritten match here and its sibling in
/// the TUI's `UiCommandSink` into a single dispatcher
/// ([`map_agent_loop_event`]). Each sink overrides only the hooks it
/// cares about; unhandled variants fall through to no-op defaults,
/// but the dispatcher's match is exhaustive, so adding a new
/// [`AgentLoopEvent`] variant is still a compile error until every
/// consumer has handled it.
///
/// The WS sink's send path is a best-effort `try_send`: when the
/// outbound mpsc is full or closed we flip `closed` so the driver
/// loop can drop out. This matches the pre-consolidation behaviour of
/// `break`-ing on the first failed send.
struct OutboundMessageSink<'a> {
    outbound: &'a mpsc::Sender<OutboundMessage>,
    closed: bool,
}

impl OutboundMessageSink<'_> {
    fn push(&mut self, msg: OutboundMessage) {
        if self.closed {
            return;
        }
        if self.outbound.try_send(msg).is_err() {
            self.closed = true;
        }
    }
}

#[async_trait]
impl TurnEventSink for OutboundMessageSink<'_> {
    async fn on_text_delta(&mut self, text: String) {
        self.push(OutboundMessage::TextDelta(TextDelta { text }));
    }

    async fn on_thinking_delta(&mut self, thinking: String) {
        self.push(OutboundMessage::ThinkingDelta(ThinkingDelta { thinking }));
    }

    async fn on_tool_start(&mut self, id: String, name: String) {
        self.push(OutboundMessage::ToolUseStart(ToolUseStart { id, name }));
    }

    async fn on_tool_result(
        &mut self,
        tool_use_id: String,
        tool_name: String,
        content: String,
        is_error: bool,
    ) {
        self.push(OutboundMessage::ToolResult(ToolResultMsg {
            name: tool_name,
            result: content,
            is_error,
            tool_use_id: Some(tool_use_id),
        }));
    }

    async fn on_tool_input_snapshot(&mut self, id: String, name: String, input: String) {
        // While streaming with `eager_input_streaming`, `input` is
        // partial JSON like `{"title":"Hi","markdown_contents":"# H`.
        // A strict `serde_json::from_str` would fail and yield `{}`,
        // making every mid-stream snapshot useless to the UI. Use a
        // tool-aware partial-JSON extractor that pulls out the
        // best-effort value of well-known string fields the preview
        // cards consume (markdown_contents, content, old_text, etc.).
        let parsed = super::partial_json::parse_partial_tool_input(&name, &input);
        let md_len = parsed
            .get("markdown_contents")
            .and_then(|v| v.as_str())
            .map_or(0, str::len);
        let content_len = parsed
            .get("content")
            .and_then(|v| v.as_str())
            .map_or(0, str::len);
        tracing::info!(
            tool = %name,
            raw_input_bytes = input.len(),
            parsed_keys = parsed.as_object().map_or(0, |o| o.len()),
            markdown_len = md_len,
            content_len,
            "forwarding tool_call_snapshot"
        );
        self.push(OutboundMessage::ToolCallSnapshot(ToolCallSnapshot {
            id,
            name,
            input: parsed,
        }));
    }

    async fn on_error(&mut self, code: String, message: String, recoverable: bool) {
        self.push(OutboundMessage::Error(ErrorMsg {
            code,
            message,
            recoverable,
        }));
    }

    // The following variants are intentional no-ops on the WS wire —
    // `ToolComplete`, `IterationComplete`, `ThinkingComplete`,
    // `StepComplete`, `StreamReset`, `Warning`, `Debug`. The trait
    // defaults cover them, but the mapper's exhaustive match still
    // forces a decision here whenever the event enum changes.
    async fn on_debug(&mut self, _event: DebugEvent) {}
}

pub(super) async fn forward_events_to_ws(
    mut event_rx: mpsc::Receiver<AgentLoopEvent>,
    outbound: mpsc::Sender<OutboundMessage>,
) {
    let mut sink = OutboundMessageSink {
        outbound: &outbound,
        closed: false,
    };
    while let Some(event) = event_rx.recv().await {
        map_agent_loop_event(event, &mut sink).await;
        if sink.closed {
            break;
        }
    }
}

pub(super) fn finalize_turn(
    session: &mut Session,
    join_result: Result<anyhow::Result<AgentLoopResult>, tokio::task::JoinError>,
    message_id: &str,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
) {
    let result = match join_result {
        Ok(inner) => inner,
        Err(e) => {
            error!(session_id = %session.session_id, error = %e, "Turn task panicked");
            send_turn_error(outbound_tx, message_id);
            return;
        }
    };

    match result {
        Ok(loop_result) => {
            apply_turn_result(session, &loop_result, message_id, outbound_tx);
        }
        Err(e) => {
            error!(session_id = %session.session_id, error = %e, "Turn processing failed");
            let _ = outbound_tx.try_send(OutboundMessage::Error(ErrorMsg {
                code: "turn_error".into(),
                message: format!("Turn processing failed: {e}"),
                recoverable: true,
            }));
        }
    }
}

fn send_turn_error(outbound_tx: &mpsc::Sender<OutboundMessage>, message_id: &str) {
    let _ = outbound_tx.try_send(OutboundMessage::Error(ErrorMsg {
        code: "internal_error".into(),
        message: "Turn processing task panicked".into(),
        recoverable: false,
    }));
    let _ = outbound_tx.try_send(OutboundMessage::AssistantMessageEnd(AssistantMessageEnd {
        message_id: message_id.to_string(),
        stop_reason: "error".into(),
        usage: SessionUsage::default(),
        files_changed: FilesChanged::default(),
        originating_user_id: None,
    }));
}

pub(super) fn apply_turn_result(
    session: &mut Session,
    loop_result: &AgentLoopResult,
    message_id: &str,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
) {
    session.messages.clone_from(&loop_result.messages);
    // Defense-in-depth: cap any tool_use input / tool_result content
    // that exceeds `SESSION_TOOL_BLOB_MAX_BYTES`. Utilization-based
    // compaction in `aura_agent` kicks in at 15%+ of the context
    // window; a single oversized blob (e.g. a verbose `list_agents`
    // result on a cold start) can still bloat the wire payload well
    // below that floor. This per-blob cap keeps those blobs from
    // riding along with every subsequent turn's prompt.
    super::truncate_messages_for_storage(&mut session.messages);
    let files_changed = summarize_files_changed(loop_result);

    let input_tokens = loop_result.total_input_tokens;
    let output_tokens = loop_result.total_output_tokens;
    let estimated_context_tokens = loop_result.estimated_context_tokens;
    let cache_creation_input_tokens = loop_result.total_cache_creation_input_tokens;
    let cache_read_input_tokens = loop_result.total_cache_read_input_tokens;
    session.cumulative_input_tokens += input_tokens;
    session.cumulative_output_tokens += output_tokens;
    session.cumulative_cache_creation_input_tokens += cache_creation_input_tokens;
    session.cumulative_cache_read_input_tokens += cache_read_input_tokens;

    let stop_reason = if loop_result.timed_out {
        "cancelled"
    } else if loop_result.insufficient_credits {
        "insufficient_credits"
    } else if loop_result.llm_error.is_some() {
        "end_turn_with_errors"
    } else {
        "end_turn"
    };

    let context_utilization = if session.context_window_tokens > 0 {
        #[allow(clippy::cast_precision_loss)]
        let ratio = estimated_context_tokens as f32 / session.context_window_tokens as f32;
        ratio.min(1.0)
    } else {
        0.0
    };

    let _ = outbound_tx.try_send(OutboundMessage::AssistantMessageEnd(AssistantMessageEnd {
        message_id: message_id.to_string(),
        stop_reason: stop_reason.into(),
        usage: SessionUsage {
            input_tokens,
            output_tokens,
            estimated_context_tokens,
            cache_creation_input_tokens,
            cache_read_input_tokens,
            cumulative_input_tokens: session.cumulative_input_tokens,
            cumulative_output_tokens: session.cumulative_output_tokens,
            cumulative_cache_creation_input_tokens: session.cumulative_cache_creation_input_tokens,
            cumulative_cache_read_input_tokens: session.cumulative_cache_read_input_tokens,
            context_utilization,
            model: session.model.clone(),
            provider: session.provider_name.clone(),
        },
        files_changed,
        originating_user_id: None,
    }));

    info!(
        session_id = %session.session_id,
        timed_out = loop_result.timed_out,
        iterations = loop_result.iterations,
        history_len = session.messages.len(),
        "Turn complete"
    );
}

#[cfg(test)]
mod tests {
    use super::{handle_session_init, resolve_session_workspace, summarize_files_changed};
    use crate::protocol::OutboundMessage;
    use crate::session::{Session, WsContext};
    use aura_agent::{AgentLoopResult, FileChange, FileChangeKind};
    use aura_protocol::{AgentPermissionsWire, SessionInit, SessionProviderConfig};
    use aura_reasoner::MockProvider;
    use aura_store::RocksStore;
    use aura_tools::{ToolCatalog, ToolConfig};
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    fn test_context() -> WsContext {
        let workspace = tempfile::tempdir().expect("temp workspace");
        let db_dir = tempfile::tempdir().expect("temp db");
        let store = RocksStore::open(db_dir.path(), false).expect("open rocks store");
        let workspace_base = workspace.path().to_path_buf();
        std::mem::forget(workspace);
        std::mem::forget(db_dir);

        WsContext {
            workspace_base,
            provider: Arc::new(MockProvider::simple_response("ok")),
            store: Arc::new(store),
            tool_config: ToolConfig::default(),
            auth_token: None,
            catalog: Arc::new(ToolCatalog::default()),
            domain_api: None,
            automaton_controller: None,
            project_base: None,
            memory_manager: None,
            skill_manager: None,
            router_url: None,
        }
    }

    #[test]
    fn summarize_files_changed_groups_by_operation() {
        let loop_result = AgentLoopResult {
            file_changes: vec![
                FileChange {
                    path: "src/new.rs".into(),
                    kind: FileChangeKind::Create,
                },
                FileChange {
                    path: "src/lib.rs".into(),
                    kind: FileChangeKind::Modify,
                },
                FileChange {
                    path: "src/old.rs".into(),
                    kind: FileChangeKind::Delete,
                },
            ],
            ..AgentLoopResult::default()
        };

        let summary = summarize_files_changed(&loop_result);
        assert_eq!(summary.created, vec!["src/new.rs"]);
        assert_eq!(summary.modified, vec!["src/lib.rs"]);
        assert_eq!(summary.deleted, vec!["src/old.rs"]);
    }

    #[test]
    fn resolve_session_workspace_uses_project_path_directly() {
        let mut session = Session::new(PathBuf::from("/tmp/aura"));
        session.project_path = Some(PathBuf::from("/tmp/project"));

        let (workspace, use_workspace_base_as_root) = resolve_session_workspace(&session);

        assert_eq!(workspace, PathBuf::from("/tmp/project"));
        assert!(use_workspace_base_as_root);
    }

    #[test]
    fn resolve_session_workspace_uses_explicit_workspace_directly() {
        let mut session = Session::new(PathBuf::from("/tmp/aura"));
        session.workspace = PathBuf::from("/tmp/aura/session-123");

        let (workspace, use_workspace_base_as_root) = resolve_session_workspace(&session);

        assert_eq!(workspace, PathBuf::from("/tmp/aura/session-123"));
        assert!(use_workspace_base_as_root);
    }

    #[test]
    fn resolve_session_workspace_keeps_base_for_default_workspace() {
        let session = Session::new(PathBuf::from("/tmp/aura"));

        let (workspace, use_workspace_base_as_root) = resolve_session_workspace(&session);

        assert_eq!(workspace, PathBuf::from("/tmp/aura"));
        assert!(!use_workspace_base_as_root);
    }

    #[tokio::test]
    async fn failed_init_does_not_leave_provider_override_state() {
        let ctx = test_context();
        let mut session = Session::new(ctx.workspace_base.clone());
        let (outbound_tx, mut outbound_rx) = mpsc::channel(8);

        let invalid_workspace = tempfile::tempdir()
            .expect("outside workspace")
            .path()
            .join("outside");
        let invalid_init = SessionInit {
            system_prompt: None,
            model: None,
            max_tokens: None,
            temperature: None,
            max_turns: None,
            installed_tools: None,
            installed_integrations: None,
            workspace: Some(invalid_workspace.display().to_string()),
            project_path: None,
            token: None,
            project_id: None,
            conversation_messages: None,
            aura_agent_id: None,
            aura_session_id: None,
            aura_org_id: None,
            agent_id: None,
            user_id: "user-test".to_string(),
            tool_permissions: None,
            provider_config: Some(SessionProviderConfig {
                provider: "anthropic".to_string(),
                routing_mode: Some("proxy".to_string()),
                upstream_provider_family: None,
                api_key: None,
                base_url: Some("http://127.0.0.1:9999".to_string()),
                default_model: Some("claude-opus-4-6".to_string()),
                fallback_model: None,
                prompt_caching_enabled: Some(true),
            }),
            intent_classifier: None,
            agent_permissions: AgentPermissionsWire::default(),
        };

        handle_session_init(&mut session, invalid_init, &outbound_tx, &ctx).await;

        assert!(!session.initialized);
        assert!(session.provider_override.is_none());
        assert!(session.provider_name.is_empty());
        assert!(matches!(
            outbound_rx.recv().await,
            Some(OutboundMessage::Error(err)) if err.code == "invalid_workspace"
        ));

        let retry_workspace = ctx.workspace_base.join("retry-session");
        std::fs::create_dir_all(&retry_workspace).expect("retry workspace should exist");
        let retry_init = SessionInit {
            system_prompt: None,
            model: None,
            max_tokens: None,
            temperature: None,
            max_turns: None,
            installed_tools: None,
            installed_integrations: None,
            workspace: Some(retry_workspace.display().to_string()),
            project_path: None,
            token: None,
            project_id: None,
            conversation_messages: None,
            aura_agent_id: None,
            aura_session_id: None,
            aura_org_id: None,
            agent_id: None,
            user_id: "user-test".to_string(),
            tool_permissions: None,
            provider_config: None,
            intent_classifier: None,
            agent_permissions: AgentPermissionsWire::default(),
        };

        handle_session_init(&mut session, retry_init, &outbound_tx, &ctx).await;

        assert!(session.initialized);
        assert!(session.provider_override.is_none());
    }

    /// Wave 2 T2 — Invariants §2 + §11:
    ///
    /// `handle_session_init` must submit a `Transaction::session_start(...)`
    /// through the kernel so the record log reflects the session boundary
    /// and the policy's session-scoped approvals are cleared.
    ///
    /// A follow-on kernel call (what `start_turn` now does) must append a
    /// `UserPrompt` entry with the user message as payload.
    #[tokio::test]
    async fn session_init_emits_session_start_and_user_prompt_are_recorded() {
        use aura_core::{Transaction, TransactionType};
        use aura_kernel::{ExecutorRouter, Kernel, KernelConfig};

        let ctx = test_context();
        let mut session = Session::new(ctx.workspace_base.clone());

        let ws_path = ctx.workspace_base.join("record-test");
        std::fs::create_dir_all(&ws_path).unwrap();

        let init = SessionInit {
            system_prompt: None,
            model: None,
            max_tokens: None,
            temperature: None,
            max_turns: None,
            installed_tools: None,
            installed_integrations: None,
            workspace: Some(ws_path.display().to_string()),
            project_path: None,
            token: None,
            project_id: None,
            conversation_messages: None,
            aura_agent_id: None,
            aura_session_id: None,
            aura_org_id: None,
            agent_id: None,
            user_id: "user-test".to_string(),
            tool_permissions: None,
            provider_config: None,
            intent_classifier: None,
            agent_permissions: AgentPermissionsWire::default(),
        };

        let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
        handle_session_init(&mut session, init, &outbound_tx, &ctx).await;

        assert!(session.initialized);
        let agent_id = session.agent_id;

        // Drain session_ready so the channel doesn't block downstream asserts.
        let _ = outbound_rx.recv().await;

        // SessionStart must be the first recorded transaction for this agent.
        let entries = ctx.store.scan_record(agent_id, 1, 10).unwrap();
        assert!(
            entries
                .iter()
                .any(|e| e.tx.tx_type == TransactionType::SessionStart),
            "expected SessionStart entry in record, got: {:?}",
            entries.iter().map(|e| e.tx.tx_type).collect::<Vec<_>>(),
        );

        // Simulate what `start_turn` now does before invoking the agent loop:
        // build the same kernel and submit a `UserPrompt` via `process_direct`.
        let kernel = Arc::new(
            Kernel::new(
                ctx.store.clone(),
                ctx.provider.clone(),
                ExecutorRouter::new(),
                KernelConfig {
                    workspace_base: ws_path.clone(),
                    use_workspace_base_as_root: true,
                    ..KernelConfig::default()
                },
                agent_id,
            )
            .unwrap(),
        );
        kernel
            .process_direct(Transaction::user_prompt(agent_id, "hello kernel"))
            .await
            .unwrap();

        let entries = ctx.store.scan_record(agent_id, 1, 10).unwrap();
        assert!(
            entries
                .iter()
                .any(|e| e.tx.tx_type == TransactionType::UserPrompt
                    && e.tx.payload.as_ref() == b"hello kernel"),
            "expected UserPrompt entry with payload 'hello kernel', got: {:?}",
            entries.iter().map(|e| e.tx.tx_type).collect::<Vec<_>>(),
        );
    }
}
