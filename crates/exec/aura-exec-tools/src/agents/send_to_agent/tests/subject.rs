use super::*;

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
