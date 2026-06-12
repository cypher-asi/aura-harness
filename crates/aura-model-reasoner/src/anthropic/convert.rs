use super::api_types::{
    ApiComputerTool, ApiContent, ApiImageSource, ApiMessage, ApiOutputConfig, ApiThinkingConfig,
    ApiTool, ApiToolChoice, ApiToolEntry,
};
use crate::{
    ContentBlock, ImageSource, Message, ModelRequest, Role, ThinkingEffort, ToolChoice,
    ToolDefinition, ToolResultContent,
};

/// Resolve extended thinking config for a given model.
///
/// Uses the caller-supplied config when present; otherwise auto-enables
/// thinking for capable models when the token budget is large enough.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ThinkingMode {
    Adaptive,
    Enabled,
}

fn normalize_anthropic_model(model: &str) -> String {
    model
        .trim()
        .to_ascii_lowercase()
        .trim_start_matches("aura-")
        .to_string()
}

fn thinking_mode_for_model(model: &str) -> Option<ThinkingMode> {
    let model = normalize_anthropic_model(model);
    if model.starts_with("claude-opus-4") || model.starts_with("claude-sonnet-4") {
        Some(ThinkingMode::Adaptive)
    } else if model.starts_with("claude-3-7-sonnet") {
        Some(ThinkingMode::Enabled)
    } else {
        None
    }
}

pub(super) fn resolve_thinking(request: &ModelRequest, model: &str) -> Option<ApiThinkingConfig> {
    // Historical note: dev-loop requests used to escalate
    // [`ThinkingMode::Adaptive`] to [`ThinkingMode::Enabled`] to coax
    // visible `ThinkingDelta` frames out of opus-4 / sonnet-4, gated by
    // the `AURA_DEV_LOOP_ENABLED_THINKING` kill switch. Anthropic
    // removed `enabled` mode for the Claude 4 family in May 2026 —
    // requests now 400 with `"thinking.type.enabled" is not supported
    // for this model. Use "thinking.type.adaptive" and
    // "output_config.effort" to control thinking behavior.` Adaptive
    // plus `output_config.effort: "high"` (set by
    // `resolve_output_config` below) is the replacement, so the
    // escalation and its kill switch were dropped.
    let thinking_mode = thinking_mode_for_model(model)?;

    // Phase 2: explicit reasoning-effort knob takes precedence over the
    // legacy `max_tokens > 2048` auto-enable path. Codex sets
    // `reasoning.effort` per request (codex-rs/core/src/client.rs:698-714);
    // when a caller opts in via `ModelRequest::thinking_effort = Some(_)`,
    // bypass both the explicit `ThinkingConfig` honoring AND the
    // max_tokens-coupled heuristic, and use the calibrated budget that
    // matches the requested effort. `None` falls through to the legacy
    // behaviour below so non-migrated callers keep their current shape.
    if let Some(effort) = request.thinking_effort {
        return match effort {
            ThinkingEffort::Off => None,
            // Anthropic has no sub-`Low` thinking tier, so `Minimal`
            // requests the smallest budget the API accepts (in
            // `enabled` mode); in `adaptive` mode the model still picks
            // its own budget. The distinct `minimal` value is preserved
            // end-to-end for OpenAI via the neutral `reasoning_effort`
            // field on the outgoing body.
            ThinkingEffort::Minimal => Some(ApiThinkingConfig {
                thinking_type: thinking_mode_label(thinking_mode).to_string(),
                budget_tokens: match thinking_mode {
                    ThinkingMode::Adaptive => None,
                    ThinkingMode::Enabled => Some(1024),
                },
            }),
            ThinkingEffort::Low => Some(ApiThinkingConfig {
                thinking_type: thinking_mode_label(thinking_mode).to_string(),
                // Anthropic's `adaptive` thinking mode rejects
                // `budget_tokens` outright (`thinking.adaptive.budget_tokens:
                // Extra inputs are not permitted`); the model picks its
                // own budget. Only `enabled` mode (Claude 3.7) accepts a
                // budget. Mirrors the legacy branch below.
                budget_tokens: match thinking_mode {
                    ThinkingMode::Adaptive => None,
                    ThinkingMode::Enabled => Some(1024),
                },
            }),
            ThinkingEffort::Medium => Some(ApiThinkingConfig {
                thinking_type: thinking_mode_label(thinking_mode).to_string(),
                budget_tokens: match thinking_mode {
                    ThinkingMode::Adaptive => None,
                    ThinkingMode::Enabled => Some(4096),
                },
            }),
            ThinkingEffort::High => Some(ApiThinkingConfig {
                thinking_type: thinking_mode_label(thinking_mode).to_string(),
                budget_tokens: match thinking_mode {
                    ThinkingMode::Adaptive => None,
                    ThinkingMode::Enabled => {
                        Some((request.max_tokens.get() / 2).clamp(8192, 16000))
                    }
                },
            }),
            ThinkingEffort::XHigh => Some(ApiThinkingConfig {
                thinking_type: thinking_mode_label(thinking_mode).to_string(),
                budget_tokens: match thinking_mode {
                    ThinkingMode::Adaptive => None,
                    ThinkingMode::Enabled => {
                        Some((request.max_tokens.get() * 3 / 4).clamp(16000, 24000))
                    }
                },
            }),
            ThinkingEffort::Max => Some(ApiThinkingConfig {
                thinking_type: thinking_mode_label(thinking_mode).to_string(),
                budget_tokens: match thinking_mode {
                    ThinkingMode::Adaptive => None,
                    ThinkingMode::Enabled => Some(request.max_tokens.get().clamp(24000, 32000)),
                },
            }),
        };
    }

    if let Some(ref cfg) = request.thinking {
        return Some(ApiThinkingConfig {
            thinking_type: thinking_mode_label(thinking_mode).to_string(),
            budget_tokens: match thinking_mode {
                ThinkingMode::Adaptive => None,
                ThinkingMode::Enabled => Some(cfg.budget_tokens),
            },
        });
    }

    if request.max_tokens.get() > 2048 {
        Some(ApiThinkingConfig {
            thinking_type: thinking_mode_label(thinking_mode).to_string(),
            budget_tokens: match thinking_mode {
                ThinkingMode::Adaptive => None,
                ThinkingMode::Enabled => Some((request.max_tokens.get() / 2).clamp(1024, 16000)),
            },
        })
    } else {
        None
    }
}

fn thinking_mode_label(mode: ThinkingMode) -> &'static str {
    match mode {
        ThinkingMode::Adaptive => "adaptive",
        ThinkingMode::Enabled => "enabled",
    }
}

pub(super) fn resolve_output_config(
    request: &ModelRequest,
    model: &str,
) -> Option<ApiOutputConfig> {
    let thinking = resolve_thinking(request, model)?;
    if thinking.thinking_type != "adaptive" {
        return None;
    }
    // Phase 2: only force `output_config.effort = "high"` when the
    // caller explicitly opted into [`ThinkingEffort::High`] (or the
    // higher user tiers XHigh / Max), or when the legacy auto-enable
    // path fired (`thinking_effort: None`). Low / Medium / Off opt-in
    // callers must NOT inherit the forced-high effort — that's exactly
    // the override that amplifies the doom loop's read iterations.
    // Adaptive mode currently exposes only `"high"` as a discrete
    // effort, so XHigh / Max fold into it until the API offers finer
    // tiers.
    match request.thinking_effort {
        Some(
            ThinkingEffort::Off
            | ThinkingEffort::Minimal
            | ThinkingEffort::Low
            | ThinkingEffort::Medium,
        ) => None,
        Some(ThinkingEffort::High | ThinkingEffort::XHigh | ThinkingEffort::Max) | None => {
            Some(ApiOutputConfig {
                effort: "high".to_string(),
            })
        }
    }
}

/// Build the system block as a JSON array, optionally adding `cache_control`.
///
/// Returns `None` when `system_prompt` is empty (or whitespace-only). The
/// caller must omit the `system` field from the outgoing Anthropic request
/// in that case, because:
///
/// * `[{ "type":"text", "text":"", "cache_control":{...} }]` is rejected
///   with `system.0: cache_control cannot be set for empty text blocks`,
///   and
/// * even without `cache_control`, an empty `text` block is wasteful and
///   easy for the API to reject in the future.
///
/// Chat sessions start with `system_prompt = ""` (see
/// [`crates/aura-runtime/src/session/state.rs`] `Session::new`), so this
/// guard is reached on real production traffic.
pub(super) fn build_system_block(
    system_prompt: &str,
    prompt_caching_enabled: bool,
) -> Option<serde_json::Value> {
    if system_prompt.trim().is_empty() {
        return None;
    }
    if prompt_caching_enabled {
        Some(serde_json::json!([{
            "type": "text",
            "text": system_prompt,
            "cache_control": {"type": "ephemeral"}
        }]))
    } else {
        Some(serde_json::json!([{
            "type": "text",
            "text": system_prompt
        }]))
    }
}

pub(super) fn convert_messages_to_api(
    messages: &[Message],
    prompt_caching_enabled: bool,
) -> Vec<ApiMessage> {
    let mut api_messages: Vec<ApiMessage> = messages
        .iter()
        .map(|msg| {
            let role = match msg.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };

            let content: Vec<ApiContent> = msg
                .content
                .iter()
                .map(|block| match block {
                    ContentBlock::Text { text } => ApiContent::Text {
                        text: text.clone(),
                        cache_control: None,
                    },
                    ContentBlock::ToolUse { id, name, input } => ApiContent::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    },
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        let content_text = match content {
                            ToolResultContent::Text(s) => s.clone(),
                            ToolResultContent::Json(v) => {
                                serde_json::to_string(v).unwrap_or_default()
                            }
                        };
                        ApiContent::ToolResult {
                            tool_use_id: tool_use_id.clone(),
                            content: content_text,
                            is_error: Some(*is_error),
                            cache_control: None,
                        }
                    }
                    ContentBlock::Thinking {
                        thinking,
                        signature,
                    } => ApiContent::Thinking {
                        thinking: thinking.clone(),
                        signature: signature.clone(),
                    },
                    ContentBlock::Image { source } => ApiContent::Image {
                        source: ApiImageSource {
                            source_type: source.source_type.clone(),
                            media_type: source.media_type.clone(),
                            data: source.data.clone(),
                        },
                    },
                })
                .collect();

            ApiMessage {
                role: role.to_string(),
                content,
            }
        })
        .collect();

    if prompt_caching_enabled {
        if let Some(last_user) = api_messages.iter_mut().rev().find(|m| m.role == "user") {
            if let Some(last_block) = last_user.content.last_mut() {
                let ephemeral = serde_json::json!({"type": "ephemeral"});
                match last_block {
                    ApiContent::Text { cache_control, .. }
                    | ApiContent::ToolResult { cache_control, .. } => {
                        *cache_control = Some(ephemeral);
                    }
                    _ => {}
                }
            }
        }
    }

    dedupe_tool_results(&mut api_messages);
    drop_misplaced_tool_results(&mut api_messages);
    inject_missing_tool_results(&mut api_messages);

    api_messages
}

/// Final wire-level guard for the Anthropic positional pairing rule: every
/// `tool_result` block in message `i` must reference a `tool_use` block in
/// message `i - 1`. Violating blocks are dropped (and logged) instead of
/// shipped, because the API rejects the entire request with 400
/// "unexpected `tool_use_id` found in `tool_result` blocks" otherwise.
///
/// Upstream sanitization (`aura-agent`'s `validate_and_repair`) repairs the
/// transcript properly — including injecting synthetic results for any
/// `tool_use` left unpaired — so this guard firing means an upstream path
/// produced an invalid layout; the error log is the signal to fix it there.
pub(super) fn drop_misplaced_tool_results(api_messages: &mut Vec<ApiMessage>) {
    use std::collections::HashSet;

    let mut i = 0;
    while i < api_messages.len() {
        let previous_tool_use_ids: HashSet<String> = if i == 0 {
            HashSet::new()
        } else {
            api_messages[i - 1]
                .content
                .iter()
                .filter_map(|block| match block {
                    ApiContent::ToolUse { id, .. } => Some(id.clone()),
                    _ => None,
                })
                .collect()
        };

        api_messages[i].content.retain(|block| match block {
            ApiContent::ToolResult { tool_use_id, .. } => {
                let keep = previous_tool_use_ids.contains(tool_use_id);
                if !keep {
                    tracing::error!(
                        message_index = i,
                        tool_use_id = %tool_use_id,
                        "convert_messages_to_api: dropped tool_result without matching tool_use \
                         in the previous message; upstream history construction is invalid"
                    );
                }
                keep
            }
            _ => true,
        });

        if api_messages[i].content.is_empty() {
            // Removing the emptied message changes who the next message's
            // predecessor is, so do not advance: re-evaluate index `i`
            // against its new neighbor on the next pass.
            api_messages.remove(i);
        } else {
            i += 1;
        }
    }
}

/// Companion to [`drop_misplaced_tool_results`]: ensure every assistant
/// `tool_use` has a matching `tool_result` at the front of the immediately
/// following user message, injecting synthetic error results when missing.
/// Without this, dropping a misplaced result would just trade one 400
/// ("unexpected tool_use_id in tool_result") for another ("tool_use ids were
/// found without tool_result blocks immediately after").
pub(super) fn inject_missing_tool_results(api_messages: &mut Vec<ApiMessage>) {
    use std::collections::HashSet;

    let mut i = 0;
    while i < api_messages.len() {
        let tool_use_ids: Vec<String> = if api_messages[i].role == "assistant" {
            api_messages[i]
                .content
                .iter()
                .filter_map(|block| match block {
                    ApiContent::ToolUse { id, .. } => Some(id.clone()),
                    _ => None,
                })
                .collect()
        } else {
            Vec::new()
        };

        if tool_use_ids.is_empty() {
            i += 1;
            continue;
        }

        let existing_result_ids: HashSet<String> = api_messages
            .get(i + 1)
            .filter(|m| m.role == "user")
            .map(|m| {
                m.content
                    .iter()
                    .filter_map(|block| match block {
                        ApiContent::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();

        let synthetic: Vec<ApiContent> = tool_use_ids
            .into_iter()
            .filter(|id| !existing_result_ids.contains(id))
            .map(|id| {
                tracing::error!(
                    message_index = i,
                    tool_use_id = %id,
                    "convert_messages_to_api: injected synthetic tool_result for unpaired tool_use"
                );
                ApiContent::ToolResult {
                    tool_use_id: id,
                    content: "[Tool result was lost during context compaction]".to_string(),
                    is_error: Some(true),
                    cache_control: None,
                }
            })
            .collect();

        if !synthetic.is_empty() {
            if i + 1 < api_messages.len() && api_messages[i + 1].role == "user" {
                for (offset, block) in synthetic.into_iter().enumerate() {
                    api_messages[i + 1].content.insert(offset, block);
                }
            } else {
                api_messages.insert(
                    i + 1,
                    ApiMessage {
                        role: "user".to_string(),
                        content: synthetic,
                    },
                );
            }
        }

        i += 1;
    }
}

pub(super) fn convert_tools_to_api(
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
            last_tool.cache_control = Some(serde_json::json!({"type": "ephemeral"}));
        }
    }

    api_tools
}

/// Canonical name of the computer-use tool. Its presence in a request's
/// tool list is the single signal that flips the request into
/// computer-use mode (special tool entry + beta header).
pub(super) const COMPUTER_TOOL_NAME: &str = "computer";

/// Anthropic computer-use beta header value (the `computer_20250124`
/// tool type ships under this beta).
pub(super) const COMPUTER_USE_BETA: &str = "computer-use-2025-01-24";

/// Virtual display geometry advertised to Anthropic's computer-use tool.
/// Mirrors the desktop executor's capture surface so model-emitted
/// coordinates land on the same pixel grid the executor renders.
const COMPUTER_DISPLAY_WIDTH_PX: u32 = 1280;
const COMPUTER_DISPLAY_HEIGHT_PX: u32 = 800;
const COMPUTER_DISPLAY_NUMBER: u32 = 1;

/// True when `tools` carries the computer-use tool. Drives both the
/// special tool entry ([`convert_tool_entries_to_api`]) and the beta
/// header in the provider.
pub(super) fn request_uses_computer_tool(tools: &[ToolDefinition]) -> bool {
    tools.iter().any(|t| t.name == COMPUTER_TOOL_NAME)
}

/// Build the outbound `tools` array, swapping the harness's `computer`
/// custom-tool definition for Anthropic's built-in `computer_20250124`
/// server tool entry. Every other tool is forwarded unchanged, so
/// requests without a `computer` tool serialize byte-identically to the
/// pre-computer-use contract.
pub(super) fn convert_tool_entries_to_api(
    tools: &[ToolDefinition],
    prompt_caching_enabled: bool,
) -> Vec<ApiToolEntry> {
    convert_tools_to_api(tools, prompt_caching_enabled)
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

pub(super) fn convert_tool_choice(
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

/// Collapse duplicate `tool_result` blocks so the request honors Anthropic's
/// invariant that each `tool_use_id` may appear in at most one `tool_result`
/// block across the entire `messages[]` array.
///
/// Anthropic rejects the whole conversation with
/// `each tool_use must have a single result. Found multiple tool_result
/// blocks with id: <toolu_…>` when this rule is violated. Duplicates have
/// been observed slipping into the outbound queue from upstream recovery
/// paths (most notably `handle_max_tokens` synthesizing a placeholder for a
/// pending tool that later receives the real result), so this acts as a
/// last-line safety net before the body is serialized.
///
/// Semantics (mirrors `dedupe_tool_results_by_id` in `aura-os`'s
/// `compaction.rs`, but operates array-wide on typed [`ApiContent`]):
///
/// * **Last-write-wins on the body**: the kept block's `content`,
///   `is_error`, and `cache_control` come from the *last* occurrence of the
///   id, because that is the freshest observation.
/// * **Kept-in-place at the first occurrence**: the surviving block stays
///   at the position of the *first* occurrence, so the model still sees
///   results in the timeline order they were originally reported.
/// * **Empty messages are dropped**: if a message's only blocks were
///   duplicate `ToolResult`s, the now-empty message is removed because
///   Anthropic also 400s on empty `content` arrays.
/// * **Blocks without a `tool_use_id` pass through**: defensive guard for a
///   construction that shouldn't be reachable for `ToolResult`. We
///   deliberately do not silently drop these — the API rejecting them
///   loudly is the desired forensic signal.
/// * Non-`ToolResult` blocks and ids that appear exactly once are
///   untouched.
///
/// Emits one `tracing::warn!` per duplicated id (with the id and the count
/// of removed copies) when the sweep actually fires, so the upstream
/// emission path is easy to find in production logs.
pub(super) fn dedupe_tool_results(api_messages: &mut Vec<ApiMessage>) {
    use std::collections::{HashMap, HashSet};

    let mut positions_by_id: HashMap<String, Vec<(usize, usize)>> = HashMap::new();

    for (mi, msg) in api_messages.iter().enumerate() {
        for (ci, block) in msg.content.iter().enumerate() {
            if let ApiContent::ToolResult { tool_use_id, .. } = block {
                if tool_use_id.is_empty() {
                    continue;
                }
                positions_by_id
                    .entry(tool_use_id.clone())
                    .or_default()
                    .push((mi, ci));
            }
        }
    }

    let mut to_remove: HashSet<(usize, usize)> = HashSet::new();

    for (id, positions) in &positions_by_id {
        if positions.len() <= 1 {
            continue;
        }
        let copies_removed = positions.len() - 1;
        tracing::warn!(
            tool_use_id = %id,
            copies_removed = copies_removed,
            "convert_messages_to_api: deduplicated tool_result blocks before sending to Anthropic; \
             upstream emission path is likely synthesizing a placeholder that later collides with a real result"
        );

        let (first_mi, first_ci) = positions[0];
        let (last_mi, last_ci) = positions[positions.len() - 1];

        let (last_content, last_is_error, last_cache_control) =
            match &api_messages[last_mi].content[last_ci] {
                ApiContent::ToolResult {
                    content,
                    is_error,
                    cache_control,
                    ..
                } => (content.clone(), *is_error, cache_control.clone()),
                _ => continue,
            };

        if let ApiContent::ToolResult {
            content,
            is_error,
            cache_control,
            ..
        } = &mut api_messages[first_mi].content[first_ci]
        {
            *content = last_content;
            *is_error = last_is_error;
            *cache_control = last_cache_control;
        }

        for pos in positions.iter().skip(1) {
            to_remove.insert(*pos);
        }
    }

    if to_remove.is_empty() {
        return;
    }

    for (mi, msg) in api_messages.iter_mut().enumerate() {
        let mut indices_to_remove: Vec<usize> = to_remove
            .iter()
            .filter_map(|&(m, c)| if m == mi { Some(c) } else { None })
            .collect();
        indices_to_remove.sort_unstable_by(|a, b| b.cmp(a));
        for ci in indices_to_remove {
            msg.content.remove(ci);
        }
    }

    api_messages.retain(|m| !m.content.is_empty());
}

pub(super) fn convert_response_to_aura(content: &[ApiContent]) -> Message {
    let blocks: Vec<ContentBlock> = content
        .iter()
        .map(|c| match c {
            ApiContent::Text { text, .. } => ContentBlock::Text { text: text.clone() },
            ApiContent::Thinking {
                thinking,
                signature,
            } => ContentBlock::Thinking {
                thinking: thinking.clone(),
                signature: signature.clone(),
            },
            ApiContent::ToolUse { id, name, input } => ContentBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            },
            ApiContent::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            } => ContentBlock::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content: ToolResultContent::Text(content.clone()),
                is_error: is_error.unwrap_or(false),
            },
            ApiContent::Image { source } => ContentBlock::Image {
                source: ImageSource {
                    source_type: source.source_type.clone(),
                    media_type: source.media_type.clone(),
                    data: source.data.clone(),
                },
            },
        })
        .collect();

    Message {
        role: Role::Assistant,
        content: blocks,
    }
}
