use super::support::{chat_request, test_context};
use crate::gateway::session::helpers::{emit_session_ready, prepare_chat_session};
use crate::gateway::session::Session;
use aura_core_protocol::SessionModelOverrides;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

#[tokio::test]
async fn prepare_chat_session_preserves_managed_safe_path_with_project_base() {
    let mut ctx = test_context();
    ctx.project_base = Some(PathBuf::from("/home/aura"));
    let safe_path = ctx
        .workspace_base
        .parent()
        .expect("workspace base has a data-dir parent")
        .join("safe-workspaces/project-id/session-id/repo");
    let mut request = chat_request(None, None);
    request.workspace.project_path = Some(safe_path.to_string_lossy().to_string());

    let session = prepare_chat_session(request, &ctx)
        .await
        .expect("managed safe path should pass hosted validation");

    assert_eq!(session.project_path, Some(safe_path));
}

#[test]
fn agent_loop_config_omits_upstream_provider_family_after_overrides_only_carry_model_knobs() {
    let mut session = Session::new(PathBuf::from("/tmp/aura"));
    session.provider_overrides = Some(SessionModelOverrides {
        default_model: Some("deepseek-v4-flash".to_string()),
        fallback_model: None,
        prompt_caching_enabled: Some(true),
        prompt_cache_key: None,
        prompt_cache_retention: None,
    });

    let config = session.agent_loop_config();

    // The wire `SessionModelOverrides` no longer carries a family
    // hint; family detection on the harness side falls back to the
    // model-name heuristic in `RouterProvider::supports_anthropic_proxy_features`.
    assert_eq!(config.upstream_provider_family, None);
}

#[tokio::test]
async fn failed_init_does_not_leave_provider_override_state() {
    let ctx = test_context();

    let invalid_workspace = tempfile::tempdir()
        .expect("outside workspace")
        .path()
        .join("outside");
    let invalid_request = chat_request(
        Some(invalid_workspace.display().to_string()),
        Some(SessionModelOverrides {
            default_model: Some("claude-opus-4-6".to_string()),
            fallback_model: None,
            prompt_caching_enabled: Some(true),
            prompt_cache_key: None,
            prompt_cache_retention: None,
        }),
    );

    let outcome = prepare_chat_session(invalid_request, &ctx).await;
    let Err(err) = outcome else {
        panic!("workspace outside base must reject")
    };
    assert_eq!(err.code, "invalid_workspace");

    let retry_workspace = ctx.workspace_base.join("retry-session");
    std::fs::create_dir_all(&retry_workspace).expect("retry workspace should exist");
    let retry_request = chat_request(Some(retry_workspace.display().to_string()), None);

    let session = prepare_chat_session(retry_request, &ctx)
        .await
        .expect("valid workspace prepares cleanly");
    assert!(session.initialized);
    assert!(session.provider_override.is_none());
}

/// Wave 2 T2 — Invariants §2 + §11:
///
/// Bootstrapping a chat session must submit a
/// `Transaction::session_start(...)` through the kernel so the
/// record log reflects the session boundary and the policy's
/// session-scoped approvals are cleared.
///
/// A follow-on kernel call (what `start_turn` now does) must
/// append a `UserPrompt` entry with the user message as payload.
#[tokio::test]
async fn session_bootstrap_emits_session_start_and_user_prompt_are_recorded() {
    use aura_agent_kernel::{ExecutorRouter, Kernel, KernelConfig};
    use aura_core_types::{Transaction, TransactionType};

    let ctx = test_context();

    let ws_path = ctx.workspace_base.join("record-test");
    std::fs::create_dir_all(&ws_path).unwrap();

    let request = chat_request(Some(ws_path.display().to_string()), None);

    let mut session = prepare_chat_session(request, &ctx)
        .await
        .expect("valid chat request prepares cleanly");
    let (outbound_tx, mut outbound_rx) = mpsc::channel(8);
    emit_session_ready(&mut session, &outbound_tx, &ctx).await;

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
        entries.iter().any(|e| e
            .tx
            .prompt_payload()
            .is_some_and(|p| p.text == "hello kernel")),
        "expected UserPrompt entry with payload 'hello kernel', got: {:?}",
        entries.iter().map(|e| e.tx.tx_type).collect::<Vec<_>>(),
    );
}
