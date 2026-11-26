//! The `tools` array (custom tools and the computer-use server tool) and
//! `tool_choice`.

use super::{cache_marker, capability_model};
use crate::anthropic::api_types::{ApiComputerTool, ApiTool, ApiToolChoice, ApiToolEntry};
use crate::{ToolChoice, ToolDefinition};

pub(in crate::anthropic) fn convert_tools_to_api(
    tools: &[ToolDefinition],
    prompt_caching_enabled: bool,
) -> Vec<ApiTool> {
    let has_any_cache_control = tools.iter().any(|t| t.cache_control.is_some());

    let mut api_tools: Vec<ApiTool> = tools
        .iter()
        .map(|tool| ApiTool {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
            cache_control: tool
                .cache_control
                .as_ref()
                .map(|cc| serde_json::json!({"type": cc.cache_type})),
            eager_input_streaming: tool.eager_input_streaming,
        })
        .collect();

    if prompt_caching_enabled && !has_any_cache_control {
        if let Some(last_tool) = api_tools.last_mut() {
            last_tool.cache_control = Some(cache_marker());
        }
    }

    api_tools
}

/// Canonical name of the computer-use tool. Its presence in a request's
/// tool list is the single signal that flips the request into
/// computer-use mode (special tool entry + beta header).
pub(in crate::anthropic) const COMPUTER_TOOL_NAME: &str = "computer";

/// Anthropic computer-use beta header value (the `computer_20250124`
/// tool type ships under this beta).
pub(in crate::anthropic) const COMPUTER_USE_BETA: &str = "computer-use-2025-01-24";

/// Virtual display geometry advertised to Anthropic's computer-use tool.
/// Mirrors the desktop executor's capture surface so model-emitted
/// coordinates land on the same pixel grid the executor renders.
const COMPUTER_DISPLAY_WIDTH_PX: u32 = 1280;
const COMPUTER_DISPLAY_HEIGHT_PX: u32 = 800;
const COMPUTER_DISPLAY_NUMBER: u32 = 1;

/// True when `tools` carries the computer-use tool. Drives both the
/// special tool entry ([`convert_tool_entries_to_api`]) and the beta
/// header in the provider.
pub(in crate::anthropic) fn request_uses_computer_tool(tools: &[ToolDefinition]) -> bool {
    tools.iter().any(|t| t.name == COMPUTER_TOOL_NAME)
}

/// Build the outbound `tools` array, swapping the harness's `computer`
/// custom-tool definition for Anthropic's built-in `computer_20250124`
/// server tool entry. Every other tool is forwarded unchanged, so
/// requests without a `computer` tool serialize byte-identically to the
/// pre-computer-use contract.
///
/// A `salt` ([`crate::ModelRequest::cache_salt`]) closes the description
/// of the first tool that keeps one: the tool list is the first prefix
/// Anthropic caches, so differently salted requests share no entry.
pub(in crate::anthropic) fn convert_tool_entries_to_api(
    tools: &[ToolDefinition],
    prompt_caching_enabled: bool,
    salt: Option<&str>,
) -> Vec<ApiToolEntry> {
    let mut api_tools = convert_tools_to_api(tools, prompt_caching_enabled);
    if let Some(salt) = salt {
        if let Some(tool) = api_tools.iter_mut().find(|t| t.name != COMPUTER_TOOL_NAME) {
            tool.description =
                crate::cache_scope::closing(&tool.description, Some(salt)).into_owned();
        }
    }
    api_tools
        .into_iter()
        .map(|tool| {
            if tool.name == COMPUTER_TOOL_NAME {
                ApiToolEntry::Computer(ApiComputerTool {
                    tool_type: "computer_20250124",
                    name: "computer",
                    display_width_px: COMPUTER_DISPLAY_WIDTH_PX,
                    display_height_px: COMPUTER_DISPLAY_HEIGHT_PX,
                    display_number: COMPUTER_DISPLAY_NUMBER,
                })
            } else {
                ApiToolEntry::Custom(tool)
            }
        })
        .collect()
}

pub(in crate::anthropic) fn convert_tool_choice(
    choice: &ToolChoice,
    parallel_tool_use: bool,
) -> Option<ApiToolChoice> {
    // Phase 3: codex enables `parallel_tool_calls: true` by default
    // (codex-rs/core/src/client.rs:759); Anthropic's analog is the
    // negative-sense `disable_parallel_tool_use: bool` field on
    // `tool_choice`. We only set it when the caller opts out of
    // parallel mode — `None` (skipped during serialization) is
    // wire-equivalent to Anthropic's default-parallel behaviour and
    // keeps the request body byte-identical to the pre-Phase-3
    // shape for the default `parallel_tool_use: true` path.
    let disable = if parallel_tool_use { None } else { Some(true) };
    match choice {
        ToolChoice::Auto => Some(ApiToolChoice::Auto {
            disable_parallel_tool_use: disable,
        }),
        ToolChoice::None => None,
        ToolChoice::Required => Some(ApiToolChoice::Any {
            disable_parallel_tool_use: disable,
        }),
        ToolChoice::Tool { name } => Some(ApiToolChoice::Tool {
            name: name.clone(),
            disable_parallel_tool_use: disable,
        }),
    }
}

/// The `tool_choice` `model` is sent for `choice`, as
/// [`convert_tool_choice`] gives it, except that a forced call goes as
/// `auto` to a model that takes none
/// ([`crate::model_catalog::takes_forced_tool_choice`]): the API would
/// reject the whole request.
pub(in crate::anthropic) fn tool_choice_for(
    choice: &ToolChoice,
    parallel_tool_use: bool,
    model: &str,
) -> Option<ApiToolChoice> {
    let forced = matches!(choice, ToolChoice::Required | ToolChoice::Tool { .. });
    if forced && !crate::model_catalog::takes_forced_tool_choice(capability_model(model)) {
        tracing::warn!(%model, ?choice, "The model takes no forced tool choice; asking with auto");
        return convert_tool_choice(&ToolChoice::Auto, parallel_tool_use);
    }
    convert_tool_choice(choice, parallel_tool_use)
}
