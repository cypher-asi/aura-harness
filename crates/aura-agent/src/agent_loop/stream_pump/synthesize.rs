//! Reassemble streamed blocks without losing the provider's termination signal.
use crate::types::ToolCallInfo;
use aura_model_reasoner::{
    ContentBlock, Message, ModelResponse, ProviderTrace, Role, StopReason, Usage,
};

pub(super) fn synthesize_response(
    text_chunks: &[String],
    thinking_chunks: &[(String, Option<String>)],
    tool_calls: &[ToolCallInfo],
    end_turn: Option<bool>,
    stop_reason: Option<StopReason>,
    usage: &Usage,
    model_name: &str,
) -> ModelResponse {
    let mut content = Vec::new();
    for (thinking, signature) in thinking_chunks {
        content.push(ContentBlock::Thinking {
            thinking: thinking.clone(),
            signature: signature.clone(),
        });
    }
    for text in text_chunks {
        content.push(ContentBlock::Text { text: text.clone() });
    }
    for call in tool_calls {
        content.push(ContentBlock::ToolUse {
            id: call.id.clone(),
            name: call.name.clone(),
            input: call.input.clone(),
        });
    }
    let reason = if stop_reason == Some(StopReason::MaxTokens) {
        StopReason::MaxTokens
    } else if !tool_calls.is_empty() {
        // Preserve admitted tools even on contradictory legacy terminal frames.
        StopReason::ToolUse
    } else {
        stop_reason.unwrap_or_else(|| {
            if !tool_calls.is_empty() || end_turn == Some(false) {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            }
        })
    };
    ModelResponse::new(
        reason,
        Message::new(Role::Assistant, content),
        usage.clone(),
        ProviderTrace::new(model_name, 0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn truncation_is_preserved_even_with_completed_tools() {
        let calls = vec![ToolCallInfo {
            id: "once".into(),
            name: "write_file".into(),
            input: serde_json::json!({}),
        }];
        for tools in [&[][..], &calls[..]] {
            assert_eq!(
                synthesize_response(
                    &[],
                    &[],
                    tools,
                    Some(false),
                    Some(StopReason::MaxTokens),
                    &Usage::default(),
                    "test"
                )
                .stop_reason,
                StopReason::MaxTokens
            );
        }
    }
}
