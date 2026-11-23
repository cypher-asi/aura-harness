//! Tags thinking signatures returned by an OpenRouter-origin provider.
//!
//! Outgoing conversion only replays thinking whose signature carries
//! [`OPENROUTER_REASONING_PREFIX`] to OpenRouter (and never to other
//! backends), so everything OpenRouter returns must be tagged here.
//! Unsigned thinking stays unsigned and is dropped on the next turn.

use futures_util::StreamExt;

use super::{ReasoningOrigin, OPENROUTER_REASONING_PREFIX};
use crate::{ContentBlock, ModelResponse, StreamContentType, StreamEvent, StreamEventStream};

/// Prefix every signed thinking block in a buffered response.
pub(super) fn tag_response(response: &mut ModelResponse, origin: ReasoningOrigin) {
    if origin != ReasoningOrigin::OpenRouter {
        return;
    }
    for block in &mut response.message.content {
        if let ContentBlock::Thinking {
            signature: Some(signature),
            ..
        } = block
        {
            *signature = format!("{OPENROUTER_REASONING_PREFIX}{signature}");
        }
    }
}

/// Prefix the first signature delta of each thinking block in a stream.
///
/// Signatures may arrive split across several deltas that the
/// accumulator concatenates, so only the first one gets the prefix.
pub(super) fn tag_stream(stream: StreamEventStream, origin: ReasoningOrigin) -> StreamEventStream {
    if origin != ReasoningOrigin::OpenRouter {
        return stream;
    }
    let mut awaiting_first_signature = false;
    Box::pin(stream.map(move |item| {
        let mut event = item?;
        match &mut event {
            StreamEvent::ContentBlockStart { content_type, .. } => {
                awaiting_first_signature = matches!(content_type, StreamContentType::Thinking);
            }
            StreamEvent::SignatureDelta { signature } if awaiting_first_signature => {
                awaiting_first_signature = false;
                signature.insert_str(0, OPENROUTER_REASONING_PREFIX);
            }
            _ => {}
        }
        Ok(event)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thinking_start(index: u32) -> StreamEvent {
        StreamEvent::ContentBlockStart {
            index,
            content_type: StreamContentType::Thinking,
        }
    }

    fn signature(part: &str) -> StreamEvent {
        StreamEvent::SignatureDelta {
            signature: part.to_string(),
        }
    }

    /// Run the two-block stream through the adapter and concatenate each
    /// block's signature deltas, as the accumulator does within a block.
    async fn accumulated_signatures(origin: ReasoningOrigin) -> Vec<Option<String>> {
        let events = vec![
            thinking_start(0),
            signature("ab"),
            signature("cd"),
            StreamEvent::ContentBlockStop { index: 0 },
            thinking_start(1),
            signature("ef"),
            StreamEvent::ContentBlockStop { index: 1 },
        ];
        let source: StreamEventStream = Box::pin(futures_util::stream::iter(
            events.into_iter().map(Ok::<_, crate::ReasonerError>),
        ));
        let mut stream = tag_stream(source, origin);
        let mut blocks: Vec<Option<String>> = Vec::new();
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                StreamEvent::ContentBlockStart { .. } => blocks.push(None),
                StreamEvent::SignatureDelta { signature } => {
                    let block = blocks.last_mut().unwrap();
                    block.get_or_insert_with(String::new).push_str(&signature);
                }
                _ => {}
            }
        }
        blocks
    }

    #[test]
    fn openrouter_response_prefixes_signed_thinking_only() {
        let response = |origin| {
            let mut response = crate::ModelResponse::new(
                crate::StopReason::EndTurn,
                crate::Message::new(
                    crate::Role::Assistant,
                    vec![
                        ContentBlock::Thinking {
                            thinking: "signed".to_string(),
                            signature: Some("sig1".to_string()),
                        },
                        ContentBlock::Thinking {
                            thinking: "unsigned".to_string(),
                            signature: None,
                        },
                    ],
                ),
                crate::Usage::default(),
                crate::ProviderTrace::default(),
            );
            tag_response(&mut response, origin);
            response
                .message
                .content
                .into_iter()
                .filter_map(|block| match block {
                    ContentBlock::Thinking { signature, .. } => Some(signature),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            response(ReasoningOrigin::OpenRouter),
            [Some("openrouter:sig1".to_string()), None]
        );
        assert_eq!(
            response(ReasoningOrigin::Anthropic),
            [Some("sig1".to_string()), None]
        );
    }

    #[tokio::test]
    async fn openrouter_stream_prefixes_first_signature_delta_per_block() {
        assert_eq!(
            accumulated_signatures(ReasoningOrigin::OpenRouter).await,
            [
                Some("openrouter:abcd".to_string()),
                Some("openrouter:ef".to_string())
            ]
        );
    }

    #[tokio::test]
    async fn other_origins_leave_stream_signatures_unchanged() {
        for origin in [ReasoningOrigin::Anthropic, ReasoningOrigin::Local] {
            assert_eq!(
                accumulated_signatures(origin).await,
                [Some("abcd".to_string()), Some("ef".to_string())],
                "origin {origin:?}"
            );
        }
    }
}
