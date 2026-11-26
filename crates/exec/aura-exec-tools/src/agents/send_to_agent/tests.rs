use super::*;
use crate::sandbox::Sandbox;
use crate::ToolConfig;
use async_trait::async_trait;
use aura_core_types::{AgentId, AgentPermissions, AgentScope};
use std::sync::Arc;

fn ctx(caller: AgentPermissions) -> ToolContext {
    let dir = std::env::temp_dir();
    let mut ctx = ToolContext::new(Sandbox::new(&dir).unwrap(), ToolConfig::default());
    ctx.caller_permissions = Some(caller);
    ctx.caller_agent_id = Some(AgentId::generate());
    ctx.originating_user_id = Some("user-root".into());
    ctx
}

/// In-memory `AgentControlHook` used to drive the `execute` path
/// without standing up a real aura-os-server. `deliver_message`
/// returns `Ok(())` so the success branch is exercised.
struct OkHook;

#[async_trait]
impl crate::tool::AgentControlHook for OkHook {
    async fn deliver_message(
        &self,
        _target_agent_id: &str,
        _parent_agent_id: Option<&str>,
        _originating_user_id: Option<&str>,
        _project_id: Option<&str>,
        _content: &str,
        _attachments: Option<serde_json::Value>,
        _model: Option<&str>,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn lifecycle(
        &self,
        _target_agent_id: &str,
        _parent_agent_id: Option<&str>,
        _originating_user_id: Option<&str>,
        _action: &str,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn delegate_task(
        &self,
        _target_agent_id: &str,
        _parent_agent_id: Option<&str>,
        _originating_user_id: Option<&str>,
        _task: &str,
        _context: Option<&serde_json::Value>,
        _model: Option<&str>,
    ) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn send_to_agent_requires_control_capability() {
    let caller = AgentPermissions {
        scope: AgentScope::default(),
        capabilities: vec![Capability::ReadAgent],
    };
    let input = SendToAgentInput {
        agent_id: "aa".into(),
        content: "hello".into(),
        attachments: None,
        subject: None,
    };
    let err = SendToAgentTool::evaluate(&ctx(caller), &input).unwrap_err();
    assert!(err.to_string().contains("permissions:"), "got: {err}");
    assert!(err.to_string().contains("ControlAgent"), "got: {err}");
}

#[test]
fn send_to_agent_denies_out_of_scope_target() {
    let caller = AgentPermissions {
        scope: AgentScope {
            agent_ids: vec!["allowed".into()],
            ..AgentScope::default()
        },
        capabilities: vec![Capability::ControlAgent],
    };
    let input = SendToAgentInput {
        agent_id: "not-allowed".into(),
        content: "hello".into(),
        attachments: None,
        subject: None,
    };
    let err = SendToAgentTool::evaluate(&ctx(caller), &input).unwrap_err();
    assert!(err.to_string().contains("permissions:"), "got: {err}");
    assert!(err.to_string().contains("AgentScope"), "got: {err}");
}

#[test]
fn send_to_agent_allows_in_scope_target() {
    let caller = AgentPermissions {
        scope: AgentScope {
            agent_ids: vec!["target-id".into()],
            ..AgentScope::default()
        },
        capabilities: vec![Capability::ControlAgent],
    };
    let input = SendToAgentInput {
        agent_id: "target-id".into(),
        content: "hello".into(),
        attachments: None,
        subject: None,
    };
    let outcome = SendToAgentTool::evaluate(&ctx(caller), &input).unwrap();
    assert_eq!(outcome.target_agent_id, "target-id");
    assert_eq!(outcome.originating_user_id.as_deref(), Some("user-root"));
    assert!(
        !outcome.delivered,
        "no hook wired — runtime side-effect skipped"
    );
}

#[test]
fn send_to_agent_universe_scope_allows_any_target() {
    let caller = AgentPermissions {
        scope: AgentScope::default(),
        capabilities: vec![Capability::ControlAgent],
    };
    let input = SendToAgentInput {
        agent_id: "anything".into(),
        content: "hi".into(),
        attachments: None,
        subject: None,
    };
    assert!(SendToAgentTool::evaluate(&ctx(caller), &input).is_ok());
}

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

/// Capturing hook that records the `parent_agent_id` value
/// `send_to_agent` actually shipped to the runtime side-effect
/// layer. Used by the cross-repo regression tests below to pin the
/// "ship the upstream OS UUID, not the truncated harness hash"
/// contract that lets the server's
/// `spawn_cross_agent_reply_callback` resolve the originating
/// agent's REST URL.
struct CapturingHook {
    captured_parent: std::sync::Mutex<Option<String>>,
    captured_project: std::sync::Mutex<Option<String>>,
    captured_model: std::sync::Mutex<Option<String>>,
}

impl CapturingHook {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            captured_parent: std::sync::Mutex::new(None),
            captured_project: std::sync::Mutex::new(None),
            captured_model: std::sync::Mutex::new(None),
        })
    }
    fn captured(&self) -> Option<String> {
        self.captured_parent.lock().unwrap().clone()
    }
    fn captured_model(&self) -> Option<String> {
        self.captured_model.lock().unwrap().clone()
    }
    fn captured_project(&self) -> Option<String> {
        self.captured_project.lock().unwrap().clone()
    }
}

#[async_trait]
impl crate::tool::AgentControlHook for CapturingHook {
    async fn deliver_message(
        &self,
        _target_agent_id: &str,
        parent_agent_id: Option<&str>,
        _originating_user_id: Option<&str>,
        project_id: Option<&str>,
        _content: &str,
        _attachments: Option<serde_json::Value>,
        model: Option<&str>,
    ) -> Result<(), String> {
        *self.captured_parent.lock().unwrap() = parent_agent_id.map(str::to_string);
        *self.captured_project.lock().unwrap() = project_id.map(str::to_string);
        *self.captured_model.lock().unwrap() = model.map(str::to_string);
        Ok(())
    }
    async fn lifecycle(
        &self,
        _: &str,
        _: Option<&str>,
        _: Option<&str>,
        _: &str,
    ) -> Result<(), String> {
        Ok(())
    }
    async fn delegate_task(
        &self,
        _: &str,
        _: Option<&str>,
        _: Option<&str>,
        _: &str,
        _: Option<&serde_json::Value>,
        _model: Option<&str>,
    ) -> Result<(), String> {
        Ok(())
    }
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

/// Hook with threads: records the subject `send_to_agent` passed.
#[derive(Default)]
struct ThreadedHook {
    captured: std::sync::Mutex<Option<(String, Option<String>)>>,
}

#[async_trait]
impl crate::tool::AgentControlHook for ThreadedHook {
    async fn deliver_message(
        &self,
        _: &str,
        _: Option<&str>,
        _: Option<&str>,
        _: Option<&str>,
        _: &str,
        _: Option<serde_json::Value>,
        _: Option<&str>,
    ) -> Result<(), String> {
        Err("send_to_agent must call deliver_message_in_thread".into())
    }

    async fn deliver_message_in_thread(
        &self,
        _target_agent_id: &str,
        _parent_agent_id: Option<&str>,
        _originating_user_id: Option<&str>,
        _project_id: Option<&str>,
        content: &str,
        _attachments: Option<serde_json::Value>,
        _model: Option<&str>,
        subject: Option<&str>,
    ) -> Result<(), String> {
        *self.captured.lock().unwrap() = Some((content.to_string(), subject.map(str::to_string)));
        Ok(())
    }

    async fn lifecycle(
        &self,
        _: &str,
        _: Option<&str>,
        _: Option<&str>,
        _: &str,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn delegate_task(
        &self,
        _: &str,
        _: Option<&str>,
        _: Option<&str>,
        _: &str,
        _: Option<&serde_json::Value>,
        _: Option<&str>,
    ) -> Result<(), String> {
        Ok(())
    }
}

fn control_ctx() -> ToolContext {
    ctx(AgentPermissions {
        scope: AgentScope::default(),
        capabilities: vec![Capability::ControlAgent],
    })
}

#[tokio::test]
async fn send_to_agent_forwards_the_subject() {
    let hook = Arc::new(ThreadedHook::default());
    let mut tctx = control_ctx();
    tctx.agent_control_hook = Some(hook.clone());

    let result = SendToAgentTool
        .execute(
            &tctx,
            serde_json::json!({
                "agent_id": "target-id",
                "content": "hi",
                "subject": "Parser review",
            }),
        )
        .await
        .expect("execute");

    assert!(result.ok, "{:?}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(
        hook.captured.lock().unwrap().clone(),
        Some(("hi".to_string(), Some("Parser review".to_string())))
    );
}

#[tokio::test]
async fn send_to_agent_without_a_subject_passes_none() {
    let hook = Arc::new(ThreadedHook::default());
    let mut tctx = control_ctx();
    tctx.agent_control_hook = Some(hook.clone());

    let result = SendToAgentTool
        .execute(
            &tctx,
            serde_json::json!({ "agent_id": "target-id", "content": "hi" }),
        )
        .await
        .expect("execute");

    assert!(result.ok);
    assert_eq!(
        hook.captured.lock().unwrap().clone(),
        Some(("hi".to_string(), None))
    );
}

/// A hook written before threads, with only `deliver_message`, still
/// gets the message when a subject is given.
#[tokio::test]
async fn send_to_agent_with_a_subject_reaches_a_hook_without_threads() {
    let hook = CapturingHook::new();
    let mut tctx = control_ctx();
    tctx.caller_model_id = Some("model-a".into());
    tctx.agent_control_hook = Some(hook.clone());

    let result = SendToAgentTool
        .execute(
            &tctx,
            serde_json::json!({
                "agent_id": "target-id",
                "content": "hi",
                "subject": "Parser review",
            }),
        )
        .await
        .expect("execute");

    assert!(result.ok);
    assert_eq!(hook.captured_model().as_deref(), Some("model-a"));
}

#[test]
fn the_schema_offers_a_subject() {
    let definition = SendToAgentTool::definition();
    assert_eq!(
        definition.input_schema["properties"]["subject"]["type"],
        "string"
    );
    assert!(definition.description.contains("subject"));
}
