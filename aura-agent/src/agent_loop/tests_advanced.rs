//! Advanced agent loop tests: checkpoints, stall detection, exploration compaction.

use aura_reasoner::{
    ContentBlock, Message, MockProvider, MockResponse, ToolDefinition, ToolResultContent,
};

use super::{AgentLoop, AgentLoopConfig};
use crate::types::{AgentToolExecutor, ToolCallInfo, ToolCallResult};

struct MockExecutor {
    results: Vec<ToolCallResult>,
}

#[async_trait::async_trait]
impl AgentToolExecutor for MockExecutor {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        tool_calls
            .iter()
            .zip(self.results.iter())
            .map(|(tc, r)| ToolCallResult {
                tool_use_id: tc.id.clone(),
                ..r.clone()
            })
            .collect()
    }
}

#[tokio::test]
async fn test_checkpoint_after_first_write() {
    let executor = MockExecutor {
        results: vec![ToolCallResult::success("placeholder", "wrote file")],
    };

    let provider = MockProvider::new()
        .with_response(MockResponse::tool_use(
            "tool_1",
            "write_file",
            serde_json::json!({"path": "hello.txt", "content": "hi"}),
        ))
        .with_response(MockResponse::text("Done!"));

    let config = AgentLoopConfig {
        system_prompt: "test".to_string(),
        ..AgentLoopConfig::default()
    };
    let agent = AgentLoop::new(config);
    let messages = vec![Message::user("write hello.txt")];
    let tools = vec![ToolDefinition::new(
        "write_file",
        "Write a file",
        serde_json::json!({"type": "object"}),
    )];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    let has_checkpoint = result.messages.iter().any(|msg| {
        msg.content.iter().any(|block| {
            if let ContentBlock::Text { text } = block {
                text.contains("You've made your first file change")
            } else {
                false
            }
        })
    });
    assert!(
        has_checkpoint,
        "Messages should contain the checkpoint note after first write"
    );
}

#[tokio::test]
async fn test_checkpoint_not_repeated() {
    let executor = MockExecutor {
        results: vec![
            ToolCallResult::success("placeholder", "wrote file 1"),
            ToolCallResult::success("placeholder", "wrote file 2"),
        ],
    };

    let provider = MockProvider::new()
        .with_response(MockResponse::tool_use(
            "tool_1",
            "write_file",
            serde_json::json!({"path": "a.txt", "content": "a"}),
        ))
        .with_response(MockResponse::tool_use(
            "tool_2",
            "write_file",
            serde_json::json!({"path": "b.txt", "content": "b"}),
        ))
        .with_response(MockResponse::text("All done!"));

    let config = AgentLoopConfig {
        system_prompt: "test".to_string(),
        ..AgentLoopConfig::default()
    };
    let agent = AgentLoop::new(config);
    let messages = vec![Message::user("write two files")];
    let tools = vec![ToolDefinition::new(
        "write_file",
        "Write a file",
        serde_json::json!({"type": "object"}),
    )];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    let checkpoint_count = result
        .messages
        .iter()
        .flat_map(|msg| msg.content.iter())
        .filter(|block| {
            if let ContentBlock::Text { text } = block {
                text.contains("You've made your first file change")
            } else {
                false
            }
        })
        .count();
    assert_eq!(
        checkpoint_count, 1,
        "Checkpoint message should appear exactly once"
    );
}

#[tokio::test]
async fn test_stall_terminates_loop() {
    let executor = MockExecutor {
        results: vec![ToolCallResult::error(
            "placeholder",
            "Write failed: permission denied",
        )],
    };

    let provider = MockProvider::new().with_default_response(MockResponse::tool_use(
        "tool_w",
        "fs_write",
        serde_json::json!({"path": "stuck.rs", "content": "bad code"}),
    ));

    let max_iter = 10;
    let config = AgentLoopConfig {
        max_iterations: max_iter,
        system_prompt: "test".to_string(),
        ..AgentLoopConfig::default()
    };
    let agent = AgentLoop::new(config);
    let messages = vec![Message::user("write stuck.rs")];
    let tools = vec![ToolDefinition::new(
        "fs_write",
        "Write a file",
        serde_json::json!({"type": "object"}),
    )];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    assert!(
        result.stalled,
        "Loop should have been terminated by stall detection"
    );
    assert!(
        result.iterations < max_iter,
        "Loop should terminate before max_iterations (got {})",
        result.iterations
    );
    assert_eq!(
        result.iterations,
        crate::constants::STALL_STREAK_THRESHOLD,
        "Loop should terminate after exactly STALL_STREAK_THRESHOLD iterations"
    );

    let has_stall_warning = result.messages.iter().any(|msg| {
        msg.content.iter().any(|block| {
            if let ContentBlock::Text { text } = block {
                text.contains("CRITICAL") && text.contains("stalled")
            } else {
                false
            }
        })
    });
    assert!(
        has_stall_warning,
        "Messages should contain the stall recovery warning"
    );
}

#[tokio::test]
async fn test_exploration_compact_at_two_thirds() {
    let long_content = "x".repeat(3000);
    let executor = MockExecutor {
        results: vec![ToolCallResult::success("placeholder", &long_content)],
    };

    let mut provider_builder = MockProvider::new();
    for i in 0..8 {
        provider_builder = provider_builder.with_response(MockResponse::tool_use(
            format!("t{i}"),
            "fs_read",
            serde_json::json!({"path": format!("file{i}.txt")}),
        ));
    }
    provider_builder = provider_builder.with_response(MockResponse::text("Done"));
    let provider = provider_builder;

    let config = AgentLoopConfig {
        exploration_allowance: 12,
        max_context_tokens: Some(200_000),
        system_prompt: "test".to_string(),
        ..AgentLoopConfig::default()
    };
    let agent = AgentLoop::new(config);
    let messages = vec![Message::user("read many files")];
    let tools = vec![ToolDefinition::new(
        "fs_read",
        "Read a file",
        serde_json::json!({"type": "object"}),
    )];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    assert_eq!(result.iterations, 9);

    let has_truncation = result.messages.iter().any(|msg| {
        msg.content.iter().any(|block| {
            if let ContentBlock::ToolResult { content, .. } = block {
                match content {
                    ToolResultContent::Text(t) => t.contains("content truncated"),
                    _ => false,
                }
            } else {
                false
            }
        })
    });
    assert!(
        has_truncation,
        "Exploration-triggered compaction should have truncated older tool results"
    );
}

#[tokio::test]
async fn test_no_exploration_compact_when_low() {
    let long_content = "y".repeat(3000);
    let executor = MockExecutor {
        results: vec![ToolCallResult::success("placeholder", &long_content)],
    };

    let mut provider_builder = MockProvider::new();
    for i in 0..3 {
        provider_builder = provider_builder.with_response(MockResponse::tool_use(
            format!("t{i}"),
            "fs_read",
            serde_json::json!({"path": format!("file{i}.txt")}),
        ));
    }
    provider_builder = provider_builder.with_response(MockResponse::text("Done"));
    let provider = provider_builder;

    let config = AgentLoopConfig {
        exploration_allowance: 12,
        max_context_tokens: Some(200_000),
        system_prompt: "test".to_string(),
        ..AgentLoopConfig::default()
    };
    let agent = AgentLoop::new(config);
    let messages = vec![Message::user("read a few files")];
    let tools = vec![ToolDefinition::new(
        "fs_read",
        "Read a file",
        serde_json::json!({"type": "object"}),
    )];

    let result = agent
        .run(&provider, &executor, messages, tools)
        .await
        .unwrap();

    assert_eq!(result.iterations, 4);

    let has_truncation = result.messages.iter().any(|msg| {
        msg.content.iter().any(|block| {
            if let ContentBlock::ToolResult { content, .. } = block {
                match content {
                    ToolResultContent::Text(t) => t.contains("content truncated"),
                    _ => false,
                }
            } else {
                false
            }
        })
    });
    assert!(
        !has_truncation,
        "No compaction should occur with only 3 exploration calls (threshold is 8)"
    );
}
