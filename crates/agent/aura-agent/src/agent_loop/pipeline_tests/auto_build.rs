use super::*;

#[tokio::test]
async fn pipeline_auto_build_after_write() {
    let provider = MockProvider::new()
        .with_response(MockResponse::tool_use(
            "t1",
            "write_file",
            serde_json::json!({"path": "src/main.rs", "content": "fn main() {}"}),
        ))
        .with_response(MockResponse::text("done"));

    let executor = BuildCheckExecutor;
    let agent = AgentLoop::new(default_config());
    let messages = vec![Message::user("write src/main.rs")];
    let tools = vec![write_file_tool()];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    assert_eq!(result.iterations, 2);

    let has_build_failure = result.messages.iter().any(|msg| {
        msg.content.iter().any(|block| {
            if let ContentBlock::Text { text } = block {
                text.contains("Build check failed") && text.contains("error[E0308]")
            } else {
                false
            }
        })
    });
    assert!(
        has_build_failure,
        "Messages must contain the build failure output"
    );
}
