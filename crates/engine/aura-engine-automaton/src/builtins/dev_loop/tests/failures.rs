use super::*;

#[test]
fn forwards_tool_call_failed_event() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    forward_agent_event(
        &tx,
        aura_agent::AgentLoopEvent::ToolCallFailed {
            tool_use_id: "toolu_1".to_string(),
            tool_name: "write_file".to_string(),
            reason: "retries exhausted".to_string(),
        },
        Some("task-1"),
    );

    let event = rx.try_recv().expect("ToolCallFailed must forward");
    match event {
        AutomatonEvent::ToolCallFailed {
            task_id,
            tool_use_id,
            tool_name,
            reason,
        } => {
            assert_eq!(task_id, "task-1");
            assert_eq!(tool_use_id, "toolu_1");
            assert_eq!(tool_name, "write_file");
            assert_eq!(reason, "retries exhausted");
        }
        other => panic!("expected ToolCallFailed, got: {other:?}"),
    }
}
