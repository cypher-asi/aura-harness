//! Helper functions for WebSocket session management: init, executor
//! construction, event forwarding, and turn finalization.

use super::ws_handler::populate_tool_definitions;
use super::{Session, WsContext};
use crate::executor_factory;
use crate::protocol::{
    self, AssistantMessageEnd, ErrorMsg, FilesChanged, OutboundMessage, SessionInit, SessionReady,
    SessionUsage, SkillInfo, TextDelta, ThinkingDelta, ToolCallSnapshot, ToolInfo, ToolResultMsg,
    ToolUseStart,
};
use crate::provider_factory::create_provider_from_session_config;
#[allow(deprecated)]
use aura_agent::{AgentLoopEvent, AgentLoopResult, KernelToolExecutor};
use aura_kernel::{Kernel, KernelConfig};
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

pub(super) fn handle_session_init(
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

    if let Some(provider_config) = provider_config {
        match create_provider_from_session_config(&provider_config) {
            Ok(provider) => {
                session.provider_name = provider.name().to_string();
                session.provider_override = Some(provider);
            }
            Err(e) => {
                let _ = outbound_tx.try_send(OutboundMessage::Error(ErrorMsg {
                    code: "invalid_provider_config".into(),
                    message: e.to_string(),
                    recoverable: true,
                }));
                return;
            }
        }
    }

    if let Err(e) = session.apply_init(init) {
        let _ = outbound_tx.try_send(OutboundMessage::Error(ErrorMsg {
            code: "invalid_workspace".into(),
            message: e,
            recoverable: true,
        }));
        return;
    }

    if let (Some(ref base), Some(ref pp)) = (&ctx.project_base, &session.project_path) {
        let slug = pp.file_name().and_then(|n| n.to_str()).unwrap_or("default");
        session.project_path = Some(base.join(slug));
    }

    populate_tool_definitions(session, ctx);

    let tools: Vec<ToolInfo> = session
        .tool_definitions
        .iter()
        .map(protocol::tool_info_from_definition)
        .collect();

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

#[allow(dead_code, deprecated)]
pub(super) fn build_kernel_executor(session: &Session, ctx: &WsContext) -> KernelToolExecutor {
    let domain_exec = ctx.domain_api.as_ref().map(|api| {
        use aura_tools::domain_tools::DomainToolExecutor;
        Arc::new(DomainToolExecutor::with_session_context(
            api.clone(),
            session.auth_token.clone(),
            session.project_id.clone(),
        ))
    });

    let mut resolver =
        executor_factory::build_tool_resolver(&ctx.catalog, &ctx.tool_config, domain_exec);

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

    let router = executor_factory::build_executor_router(resolver);

    let (workspace, _) = resolve_session_workspace(session);
    KernelToolExecutor::new(router, session.agent_id, workspace)
}

pub(super) fn build_kernel_with_config(
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
        ))
    });

    let mut resolver =
        executor_factory::build_tool_resolver(&ctx.catalog, tool_config, domain_exec);

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

    let router = executor_factory::build_executor_router(resolver);

    let (workspace, use_workspace_base_as_root) = resolve_session_workspace(session);

    let config = KernelConfig {
        workspace_base: workspace,
        use_workspace_base_as_root,
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

pub(super) async fn forward_events_to_ws(
    mut event_rx: mpsc::Receiver<AgentLoopEvent>,
    outbound: mpsc::Sender<OutboundMessage>,
) {
    while let Some(event) = event_rx.recv().await {
        let msg = match event {
            AgentLoopEvent::TextDelta(text) => OutboundMessage::TextDelta(TextDelta { text }),
            AgentLoopEvent::ThinkingDelta(thinking) => {
                OutboundMessage::ThinkingDelta(ThinkingDelta { thinking })
            }
            AgentLoopEvent::ToolStart { id, name } => {
                OutboundMessage::ToolUseStart(ToolUseStart { id, name })
            }
            AgentLoopEvent::ToolResult {
                tool_use_id,
                tool_name,
                content,
                is_error,
            } => OutboundMessage::ToolResult(ToolResultMsg {
                name: tool_name,
                result: content,
                is_error,
                tool_use_id: Some(tool_use_id),
            }),
            AgentLoopEvent::Error {
                code,
                message,
                recoverable,
            } => OutboundMessage::Error(ErrorMsg {
                code,
                message,
                recoverable,
            }),
            AgentLoopEvent::ToolInputSnapshot { id, name, input } => {
                let parsed = serde_json::from_str(&input).unwrap_or(serde_json::json!({}));
                OutboundMessage::ToolCallSnapshot(ToolCallSnapshot {
                    id,
                    name,
                    input: parsed,
                })
            }
            AgentLoopEvent::ToolComplete { .. }
            | AgentLoopEvent::IterationComplete { .. }
            | AgentLoopEvent::ThinkingComplete
            | AgentLoopEvent::StepComplete
            | AgentLoopEvent::StreamReset { .. }
            | AgentLoopEvent::Warning(_) => continue,
        };
        if outbound.try_send(msg).is_err() {
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
    }));
}

pub(super) fn apply_turn_result(
    session: &mut Session,
    loop_result: &AgentLoopResult,
    message_id: &str,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
) {
    session.messages.clone_from(&loop_result.messages);
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
    use super::{resolve_session_workspace, summarize_files_changed};
    use crate::session::Session;
    use aura_agent::{AgentLoopResult, FileChange, FileChangeKind};
    use std::path::PathBuf;

    #[test]
    fn summarize_files_changed_groups_by_operation() {
        let mut loop_result = AgentLoopResult::default();
        loop_result.file_changes = vec![
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
        ];

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
}
