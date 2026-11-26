use crate::anthropic::api_types::{ApiRequest, ApiResponse};
use crate::anthropic::convert::{
    convert_messages_to_api, convert_response_to_aura, convert_tool_entries_to_api,
    resolve_output_config, resolve_thinking, tool_choice_for,
};
use crate::anthropic::ReasoningOrigin;
use crate::{ModelRequest, ModelResponse, ProviderTrace, StopReason, Usage};
use tracing::{debug, info};

pub(super) fn build_api_request(
    request: &ModelRequest,
    model: &str,
    system: Option<&serde_json::Value>,
    prompt_caching_enabled: bool,
    anthropic_features_enabled: bool,
    (openai_cache_key, openai_cache_retention): (Option<String>, Option<&'static str>),
    reasoning_origin: ReasoningOrigin,
) -> ApiRequest {
    let thinking = anthropic_features_enabled
        .then(|| resolve_thinking(request, model))
        .flatten();
    let output_config = anthropic_features_enabled
        .then(|| resolve_output_config(request, model))
        .flatten();
    ApiRequest {
        model: model.to_string(),
        system: system.cloned(),
        messages: convert_messages_to_api(
            &request.messages,
            prompt_caching_enabled,
            reasoning_origin,
        ),
        tools: if request.tools.is_empty() {
            None
        } else {
            Some(convert_tool_entries_to_api(
                &request.tools,
                prompt_caching_enabled,
                request.cache_salt.as_deref(),
            ))
        },
        tool_choice: if request.tools.is_empty() {
            None
        } else {
            tool_choice_for(&request.tool_choice, request.parallel_tool_use, model)
        },
        max_tokens: request.max_tokens.get(),
        temperature: if thinking.is_some() {
            Some(1.0)
        } else {
            request.temperature.map(f32::from)
        },
        thinking,
        output_config,
        // Provider-neutral effort hint for the router. Independent of
        // Anthropic feature gating: non-Anthropic upstreams (OpenAI,
        // Fireworks) carry no `thinking`/`output_config`, so this is the
        // only channel that conveys the user's selected effort to them.
        reasoning_effort: request
            .thinking_effort
            .and_then(|effort| effort.reasoning_effort_wire()),
        prompt_cache_key: openai_cache_key,
        prompt_cache_retention: openai_cache_retention,
    }
}

pub(super) fn parse_complete_response(
    api_response: ApiResponse,
    model_idx: usize,
    request_model: &str,
    model: &str,
    latency_ms: u64,
    provider_request_id: Option<String>,
) -> ModelResponse {
    let message = convert_response_to_aura(&api_response.content);
    let stop_reason = match api_response.stop_reason.as_deref() {
        Some("tool_use") => StopReason::ToolUse,
        Some("max_tokens") => StopReason::MaxTokens,
        Some("stop_sequence") => StopReason::StopSequence,
        _ => StopReason::EndTurn,
    };

    if model_idx > 0 {
        info!(primary = %request_model, fallback = %model, "Completed with fallback model");
    }

    debug!(
        stop_reason = ?stop_reason,
        latency_ms,
        input_tokens = api_response.usage.input_tokens,
        output_tokens = api_response.usage.output_tokens,
        model_used = %model,
        "Received response from Anthropic"
    );

    let model_used = api_response.model.clone();

    ModelResponse {
        stop_reason,
        message,
        usage: Usage {
            input_tokens: api_response.usage.input_tokens,
            output_tokens: api_response.usage.output_tokens,
            cache_creation_input_tokens: api_response.usage.cache_creation_input_tokens,
            cache_read_input_tokens: api_response.usage.cache_read_input_tokens,
        },
        trace: ProviderTrace {
            message_id: Some(api_response.id),
            provider_request_id,
            plan_usage: Vec::new(),
            latency_ms,
            model: api_response.model,
        },
        model_used,
    }
}
