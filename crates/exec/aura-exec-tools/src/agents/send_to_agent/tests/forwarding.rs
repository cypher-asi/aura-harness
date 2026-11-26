use super::*;

/// Cross-repo contract: a successful `send_to_agent` execution must
/// stamp `reply_delivery=async_user_message` onto the `ToolResult`
/// metadata. This is the signal the LLM-side prompt copy keys on to
/// stop trying to read the target's reply out of the synchronous
/// body and instead wait for the follow-up `user_message` that
/// aura-os-server posts back into the caller's session when the
/// target's turn finishes.
#[tokio::test]
async fn send_to_agent_marks_successful_result_with_async_reply_metadata() {
    let caller = AgentPermissions {
        scope: AgentScope::default(),
        capabilities: vec![Capability::ControlAgent],
    };
    let mut tctx = ctx(caller);
    tctx.agent_control_hook = Some(Arc::new(OkHook));

    let result = SendToAgentTool
        .execute(
            &tctx,
            serde_json::json!({
                "agent_id": "target-id",
                "content": "hi",
            }),
        )
        .await
        .expect("execute");

    assert!(result.ok, "OkHook must produce a success result");
    assert_eq!(
        result.metadata.get("target_agent_id").map(String::as_str),
        Some("target-id"),
        "existing `target_agent_id` metadata must be preserved; got: {:?}",
        result.metadata
    );
    assert_eq!(
        result.metadata.get("reply_delivery").map(String::as_str),
        Some("async_user_message"),
        "successful send_to_agent must announce async reply delivery; got: {:?}",
        result.metadata
    );

    // The model-facing body must also spell out the async-reply
    // contract so the LLM waits for the reply to arrive on its own
    // instead of offering to poll the target's state.
    let outcome: SendToAgentOutcome = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(
        outcome.note.as_deref(),
        Some(ASYNC_REPLY_NOTE),
        "successful send_to_agent body must carry the async-reply note so the LLM \
             does not try to poll for the reply; got: {outcome:?}"
    );
}

/// The caller's resolved model id must be forwarded to the runtime
/// side-effect layer as `model`, so the target agent's turn runs on
/// a real model. Cross-agent recipients usually have no server-side
/// configured model, so a missing model leaves the recipient's
/// harness session empty and the turn fails with "model name must
/// not be empty".
#[tokio::test]
async fn send_to_agent_forwards_caller_model_id_to_hook() {
    let caller = AgentPermissions {
        scope: AgentScope::default(),
        capabilities: vec![Capability::ControlAgent],
    };
    let mut tctx = ctx(caller);
    tctx.caller_model_id = Some("aura-claude-sonnet-4-6".into());
    let hook = CapturingHook::new();
    tctx.agent_control_hook = Some(hook.clone() as Arc<dyn crate::tool::AgentControlHook>);

    let result = SendToAgentTool
        .execute(
            &tctx,
            serde_json::json!({ "agent_id": "target-id", "content": "hi" }),
        )
        .await
        .expect("execute");
    assert!(result.ok);

    assert_eq!(
        hook.captured_model().as_deref(),
        Some("aura-claude-sonnet-4-6"),
        "send_to_agent must forward the caller's model so the target turn has a model"
    );
}

/// Cross-repo regression: when the runtime has wired
/// `caller_external_agent_id` (the upstream OS UUID), `send_to_agent`
/// must ship THAT value as `parent_agent_id`, not the truncated
/// harness blake3 hex from `caller_agent_id.to_string()`. The
/// server-side `spawn_cross_agent_reply_callback` uses this id as
/// the path segment when posting the recipient's reply back into
/// the originator's session, and the route extractor parses it as
/// `Uuid` — so a 16-char hex hash silently fails with 400 and the
/// async-reply chain dies. See:
///   * `aura-os-server/src/handlers/agents/chat/cross_agent_reply.rs`
///   * `ToolContext::caller_external_agent_id` field doc.
#[tokio::test]
async fn send_to_agent_prefers_caller_external_agent_id_over_harness_hash() {
    let caller = AgentPermissions {
        scope: AgentScope::default(),
        capabilities: vec![Capability::ControlAgent],
    };
    let mut tctx = ctx(caller);
    tctx.caller_external_agent_id = Some("550e8400-e29b-41d4-a716-446655440000".into());
    let hook = CapturingHook::new();
    tctx.agent_control_hook = Some(hook.clone() as Arc<dyn crate::tool::AgentControlHook>);

    let result = SendToAgentTool
        .execute(
            &tctx,
            serde_json::json!({
                "agent_id": "target-id",
                "content": "hi",
            }),
        )
        .await
        .expect("execute");
    assert!(result.ok);

    let captured = hook.captured();
    assert_eq!(
        captured.as_deref(),
        Some("550e8400-e29b-41d4-a716-446655440000"),
        "send_to_agent must ship the upstream OS UUID as parent_agent_id, not the harness hash"
    );

    // The outcome body that the LLM observes must report the same
    // server-resolvable id so a downstream tool that reads the
    // outcome JSON also sees a valid UUID.
    let outcome: SendToAgentOutcome = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(
        outcome.parent_agent_id.as_deref(),
        Some("550e8400-e29b-41d4-a716-446655440000")
    );
}

#[tokio::test]
async fn send_to_agent_forwards_the_callers_project() {
    let hook = CapturingHook::new();
    let mut tctx = ctx(AgentPermissions {
        scope: AgentScope::default(),
        capabilities: vec![Capability::ControlAgent],
    });
    tctx.caller_project_id = Some("project-123".into());
    tctx.agent_control_hook = Some(hook.clone());

    let result = SendToAgentTool
        .execute(
            &tctx,
            serde_json::json!({ "agent_id": "target-id", "content": "hi" }),
        )
        .await
        .expect("execute");

    assert!(result.ok);
    assert_eq!(hook.captured_project().as_deref(), Some("project-123"));
}

/// Companion test: when the runtime has NOT wired
/// `caller_external_agent_id` (legacy / in-process unit-test
/// scaffolding), the tool falls back to
/// `caller_agent_id.to_string()` so older code paths keep
/// observing some non-empty parent id rather than `None`. This
/// keeps backwards compatibility with existing integration tests
/// that build `ToolContext` by hand without going through the
/// runtime executor wiring.
#[tokio::test]
async fn send_to_agent_falls_back_to_caller_agent_id_when_external_missing() {
    let caller = AgentPermissions {
        scope: AgentScope::default(),
        capabilities: vec![Capability::ControlAgent],
    };
    let mut tctx = ctx(caller);
    assert!(
        tctx.caller_external_agent_id.is_none(),
        "fixture must leave external id unset to exercise the fallback path"
    );
    let hook = CapturingHook::new();
    tctx.agent_control_hook = Some(hook.clone() as Arc<dyn crate::tool::AgentControlHook>);

    let result = SendToAgentTool
        .execute(
            &tctx,
            serde_json::json!({ "agent_id": "target-id", "content": "hi" }),
        )
        .await
        .expect("execute");
    assert!(result.ok);

    let captured = hook.captured().expect("captured parent must be Some");
    // `AgentId`'s Display truncates to 16 hex chars — the legacy
    // value we used to ship to the server.
    assert_eq!(captured.len(), 16, "fallback must be the truncated hash");
}

/// A blank/whitespace `caller_external_agent_id` must NOT clobber
/// the fallback — defensive in case some upstream sets it to ""
/// instead of clearing the field.
#[tokio::test]
async fn send_to_agent_treats_blank_external_id_as_unset() {
    let caller = AgentPermissions {
        scope: AgentScope::default(),
        capabilities: vec![Capability::ControlAgent],
    };
    let mut tctx = ctx(caller);
    tctx.caller_external_agent_id = Some("   ".into());
    let hook = CapturingHook::new();
    tctx.agent_control_hook = Some(hook.clone() as Arc<dyn crate::tool::AgentControlHook>);

    let result = SendToAgentTool
        .execute(
            &tctx,
            serde_json::json!({ "agent_id": "target-id", "content": "hi" }),
        )
        .await
        .expect("execute");
    assert!(result.ok);

    let captured = hook.captured().expect("captured parent must be Some");
    assert_eq!(
        captured.len(),
        16,
        "blank external id must be ignored and the fallback used"
    );
}
