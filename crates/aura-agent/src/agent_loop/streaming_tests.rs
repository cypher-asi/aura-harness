use aura_reasoner::{
    ContentBlock, Message, MockProvider, ModelProvider, ModelRequest, ModelResponse,
    ProviderTrace, ReasonerError, StopReason, StreamContentType, StreamEvent, StreamEventStream,
    Usage,
};
use futures_util::stream;
use tokio::sync::mpsc;

use super::{AgentLoop, AgentLoopConfig};
use crate::events::AgentLoopEvent;
use crate::types::{AgentToolExecutor, ToolCallInfo, ToolCallResult};

struct NoOpExecutor;

#[async_trait::async_trait]
impl AgentToolExecutor for NoOpExecutor {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        tool_calls
            .iter()
            .map(|tc| ToolCallResult::success(&tc.id, "ok"))
            .collect()
    }
}

struct StreamErrorProvider {
    fallback_text: String,
}

impl StreamErrorProvider {
    fn new(text: &str) -> Self {
        Self {
            fallback_text: text.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl ModelProvider for StreamErrorProvider {
    fn name(&self) -> &'static str {
        "stream-error-test"
    }

    async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        Ok(ModelResponse::new(
            StopReason::EndTurn,
            Message::assistant(&self.fallback_text),
            Usage::new(10, 5),
            ProviderTrace::new("test", 0),
        ))
    }

    async fn complete_streaming(
        &self,
        _request: ModelRequest,
    ) -> Result<StreamEventStream, ReasonerError> {
        let events = vec![
            Ok(StreamEvent::MessageStart {
                message_id: "msg_err".to_string(),
                model: "test".to_string(),
                input_tokens: Some(10),
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
            Ok(StreamEvent::ContentBlockStart {
                index: 0,
                content_type: StreamContentType::Text,
            }),
            Ok(StreamEvent::TextDelta {
                text: "partial...".to_string(),
            }),
            Err(ReasonerError::Internal("Connection lost".to_string())),
        ];
        Ok(Box::pin(stream::iter(events)))
    }

    async fn health_check(&self) -> bool {
        true
    }
}

struct SuccessStreamProvider {
    inner: MockProvider,
}

#[async_trait::async_trait]
impl ModelProvider for SuccessStreamProvider {
    fn name(&self) -> &'static str {
        "success-stream-test"
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        self.inner.complete(request).await
    }

    async fn complete_streaming(
        &self,
        request: ModelRequest,
    ) -> Result<StreamEventStream, ReasonerError> {
        let response = self.inner.complete(request).await?;
        let mut events: Vec<Result<StreamEvent, ReasonerError>> = Vec::new();

        events.push(Ok(StreamEvent::MessageStart {
            message_id: "msg_ok".to_string(),
            model: "mock-model".to_string(),
            input_tokens: Some(response.usage.input_tokens),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }));

        for (idx, block) in response.message.content.iter().enumerate() {
            let index = idx as u32;
            if let ContentBlock::Text { text } = block {
                events.push(Ok(StreamEvent::ContentBlockStart {
                    index,
                    content_type: StreamContentType::Text,
                }));
                events.push(Ok(StreamEvent::TextDelta { text: text.clone() }));
                events.push(Ok(StreamEvent::ContentBlockStop { index }));
            }
        }

        events.push(Ok(StreamEvent::MessageDelta {
            stop_reason: Some(response.stop_reason),
            output_tokens: response.usage.output_tokens,
        }));
        events.push(Ok(StreamEvent::MessageStop));

        Ok(Box::pin(stream::iter(events)))
    }

    async fn health_check(&self) -> bool {
        true
    }
}

fn default_config() -> AgentLoopConfig {
    AgentLoopConfig {
        system_prompt: "streaming test agent".to_string(),
        ..AgentLoopConfig::default()
    }
}

async fn collect_events(mut rx: mpsc::Receiver<AgentLoopEvent>) -> Vec<AgentLoopEvent> {
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
}

#[tokio::test]
async fn stream_error_emits_reset_before_fallback() {
    let provider = StreamErrorProvider::new("Complete fallback response");
    let executor = NoOpExecutor;
    let agent = AgentLoop::new(default_config());
    let (tx, rx) = mpsc::channel(1024);
    let messages = vec![Message::user("hello")];

    let result = agent
        .run_with_events(&provider, &executor, messages, vec![], Some(tx), None)
        .await
        .unwrap();

    assert_eq!(result.iterations, 1);

    let events = collect_events(rx).await;
    let reset_pos = events
        .iter()
        .position(|e| matches!(e, AgentLoopEvent::StreamReset { .. }));
    assert!(reset_pos.is_some(), "StreamReset event must be emitted");

    let has_text_after_reset = events[reset_pos.unwrap()..]
        .iter()
        .any(|e| matches!(e, AgentLoopEvent::TextDelta(_)));
    assert!(
        has_text_after_reset,
        "TextDelta must follow StreamReset with complete content"
    );
}

#[tokio::test]
async fn stream_reset_followed_by_complete_content() {
    let fallback_text = "The authoritative fallback text";
    let provider = StreamErrorProvider::new(fallback_text);
    let executor = NoOpExecutor;
    let agent = AgentLoop::new(default_config());
    let (tx, rx) = mpsc::channel(1024);
    let messages = vec![Message::user("hello")];

    agent
        .run_with_events(&provider, &executor, messages, vec![], Some(tx), None)
        .await
        .unwrap();

    let events = collect_events(rx).await;
    let reset_idx = events
        .iter()
        .position(|e| matches!(e, AgentLoopEvent::StreamReset { .. }))
        .expect("StreamReset must be present");

    let post_reset_text: String = events[reset_idx..]
        .iter()
        .filter_map(|e| match e {
            AgentLoopEvent::TextDelta(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(post_reset_text, fallback_text);
}

#[tokio::test]
async fn successful_stream_no_reset() {
    let provider = SuccessStreamProvider {
        inner: MockProvider::simple_response("Success!"),
    };
    let executor = NoOpExecutor;
    let agent = AgentLoop::new(default_config());
    let (tx, rx) = mpsc::channel(1024);
    let messages = vec![Message::user("hello")];

    agent
        .run_with_events(&provider, &executor, messages, vec![], Some(tx), None)
        .await
        .unwrap();

    let events = collect_events(rx).await;
    let has_reset = events
        .iter()
        .any(|e| matches!(e, AgentLoopEvent::StreamReset { .. }));
    assert!(
        !has_reset,
        "No StreamReset should be emitted on a successful stream"
    );
}

#[tokio::test]
async fn stream_error_emits_exactly_one_reset() {
    let provider = StreamErrorProvider::new("Fallback");
    let executor = NoOpExecutor;
    let agent = AgentLoop::new(default_config());
    let (tx, rx) = mpsc::channel(1024);
    let messages = vec![Message::user("hello")];

    agent
        .run_with_events(&provider, &executor, messages, vec![], Some(tx), None)
        .await
        .unwrap();

    let events = collect_events(rx).await;
    let reset_count = events
        .iter()
        .filter(|e| matches!(e, AgentLoopEvent::StreamReset { .. }))
        .count();
    assert_eq!(
        reset_count, 1,
        "Exactly one StreamReset should be emitted per fallback"
    );
}
