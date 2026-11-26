use crate::{ModelRequest, ThinkingEffort};
use tracing::warn;

/// Build the human-readable `thinking` row label for the request
/// block. Combines the caller's [`ThinkingEffort`] intent with the
/// wire-level `thinking.{type, budget_tokens}` actually serialized
/// into the outbound body, so the transcript reflects both the
/// configured effort knob and the resolved budget Anthropic will see.
///
/// Output shape:
///
/// - `"off"` when `has_thinking == false`.
/// - `"on(<parts>)"` otherwise, where `<parts>` is a ` · `-joined
///   subset of: the caller's effort label (`low`/`medium`/`high`),
///   the wire type (`enabled`/`adaptive`/…), and `b=<n>` for the
///   budget.
/// - `"on"` (parts empty) is only reachable if `has_thinking=true`
///   but neither effort nor wire fields are populated — defensive.
pub(super) fn format_thinking_label(
    has_thinking: bool,
    effort: Option<ThinkingEffort>,
    wire_type: Option<&str>,
    budget: Option<u64>,
) -> String {
    if !has_thinking {
        return "off".to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    match effort {
        Some(ThinkingEffort::Low) => parts.push("low".to_string()),
        Some(ThinkingEffort::Medium) => parts.push("medium".to_string()),
        Some(ThinkingEffort::High) => parts.push("high".to_string()),
        Some(ThinkingEffort::XHigh) => parts.push("xhigh".to_string()),
        Some(ThinkingEffort::Max) => parts.push("max".to_string()),
        // `Off` cannot reach this branch (would produce no thinking
        // config); `None` means the legacy max_tokens-coupled path
        // fired and there's no caller-level label to surface.
        _ => {}
    }
    if let Some(t) = wire_type {
        parts.push(t.to_string());
    }
    if let Some(b) = budget {
        parts.push(format!("b={b}"));
    }
    if parts.is_empty() {
        "on".to_string()
    } else {
        format!("on({})", parts.join(" · "))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RequestDiagnosticsSummary {
    pub(super) body_hash: String,
    pub(super) top_level_keys: String,
    pub(super) stream: bool,
    pub(super) system_bytes: usize,
    pub(super) messages_text_bytes: usize,
    pub(super) last_user_text_bytes: usize,
    pub(super) last_user_text_hash: Option<String>,
    pub(super) tools_count: usize,
    pub(super) tool_names: String,
    pub(super) tool_choice: Option<String>,
    pub(super) has_thinking: bool,
    /// Wire-level `thinking.type` (e.g. `"enabled"` / `"adaptive"`),
    /// captured straight from the serialized request body so the
    /// transcript reflects exactly what Anthropic receives — not what
    /// the caller intended pre-`resolve_thinking`.
    pub(super) thinking_type: Option<String>,
    /// Wire-level `thinking.budget_tokens`. Present only for
    /// `type == "enabled"`; the `adaptive` mode rejects this field.
    pub(super) thinking_budget_tokens: Option<u64>,
    pub(super) has_output_config: bool,
}

/// Build a redacted, content-free summary of the serialized router
/// request. This is intentionally derived from the final outbound JSON
/// bytes rather than the typed Rust request, so it reflects every
/// serialization detail that Cloudflare / aura-router actually sees.
pub(super) fn summarize_anthropic_request(body_bytes: &[u8]) -> RequestDiagnosticsSummary {
    let body_hash = stable_hash_hex(body_bytes);
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body_bytes) else {
        return RequestDiagnosticsSummary {
            body_hash,
            top_level_keys: "<invalid-json>".to_string(),
            stream: false,
            system_bytes: 0,
            messages_text_bytes: 0,
            last_user_text_bytes: 0,
            last_user_text_hash: None,
            tools_count: 0,
            tool_names: "<invalid-json>".to_string(),
            tool_choice: None,
            has_thinking: false,
            thinking_type: None,
            thinking_budget_tokens: None,
            has_output_config: false,
        };
    };

    let top_level_keys = value.as_object().map_or_else(
        || "<not-object>".to_string(),
        |obj| {
            let mut keys = obj.keys().map(String::as_str).collect::<Vec<_>>();
            keys.sort_unstable();
            keys.join(",")
        },
    );
    let stream = value
        .get("stream")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let system_bytes = text_bytes_in_value(value.get("system"));

    let mut messages_text_bytes = 0usize;
    let mut last_user_text = String::new();
    if let Some(messages) = value.get("messages").and_then(serde_json::Value::as_array) {
        for message in messages {
            messages_text_bytes += text_bytes_in_value(message.get("content"));
            if message.get("role").and_then(serde_json::Value::as_str) == Some("user") {
                last_user_text.clear();
                collect_text_fields(message.get("content"), &mut last_user_text);
            }
        }
    }
    let last_user_text_bytes = last_user_text.len();
    let last_user_text_hash =
        (!last_user_text.is_empty()).then(|| stable_hash_hex(last_user_text.as_bytes()));

    let (tools_count, tool_names) = summarize_tools(value.get("tools"));
    let tool_choice = value.get("tool_choice").map(compact_json_for_log);
    let thinking_value = value.get("thinking");
    let has_thinking = thinking_value.is_some();
    let thinking_type = thinking_value
        .and_then(|t| t.get("type"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let thinking_budget_tokens = thinking_value
        .and_then(|t| t.get("budget_tokens"))
        .and_then(serde_json::Value::as_u64);
    let has_output_config = value.get("output_config").is_some();

    RequestDiagnosticsSummary {
        body_hash,
        top_level_keys,
        stream,
        system_bytes,
        messages_text_bytes,
        last_user_text_bytes,
        last_user_text_hash,
        tools_count,
        tool_names,
        tool_choice,
        has_thinking,
        thinking_type,
        thinking_budget_tokens,
        has_output_config,
    }
}

fn summarize_tools(tools: Option<&serde_json::Value>) -> (usize, String) {
    let Some(tools) = tools.and_then(serde_json::Value::as_array) else {
        return (0, String::new());
    };

    let names = tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>();
    (tools.len(), names.join(","))
}

fn text_bytes_in_value(value: Option<&serde_json::Value>) -> usize {
    let mut text = String::new();
    collect_text_fields(value, &mut text);
    text.len()
}

fn collect_text_fields(value: Option<&serde_json::Value>, out: &mut String) {
    match value {
        Some(serde_json::Value::String(s)) => out.push_str(s),
        Some(serde_json::Value::Array(values)) => {
            for value in values {
                collect_text_fields(Some(value), out);
            }
        }
        Some(serde_json::Value::Object(obj)) => {
            if let Some(text) = obj.get("text").and_then(serde_json::Value::as_str) {
                out.push_str(text);
            }
            if let Some(content) = obj.get("content") {
                collect_text_fields(Some(content), out);
            }
        }
        _ => {}
    }
}

fn compact_json_for_log(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "<unserializable>".to_string())
}

/// Small stable hash for diagnostics. This is not cryptographic; it is
/// just a deterministic fingerprint to correlate requests across logs
/// and optional body dumps without printing body content.
pub(super) fn stable_hash_hex(bytes: &[u8]) -> String {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{hash:016x}")
}

pub(super) fn dump_request_body_if_enabled(
    model: &str,
    body_hash: &str,
    body_bytes: &[u8],
) -> Option<String> {
    let dir = std::env::var("AURA_LLM_DEBUG_REQUEST_DUMP_DIR")
        .ok()
        .filter(|value| !value.trim().is_empty())?;
    let dir_path = std::path::PathBuf::from(dir);
    if let Err(err) = std::fs::create_dir_all(&dir_path) {
        warn!(
            error = %err,
            debug_request_dump_dir = %dir_path.display(),
            "AURA_LLM_DEBUG_REQUEST_DUMP_DIR is set but could not be created"
        );
        return None;
    }

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let file_name = format!(
        "llm-request-{ts}-{}-{body_hash}.json",
        sanitize_filename_segment(model)
    );
    let file = dir_path.join(file_name);
    match std::fs::write(&file, body_bytes) {
        Ok(()) => Some(file.display().to_string()),
        Err(err) => {
            warn!(
                error = %err,
                debug_request_dump_path = %file.display(),
                "failed to write AURA_LLM_DEBUG_REQUEST_DUMP_DIR request body"
            );
            None
        }
    }
}

pub(super) fn sanitize_filename_segment(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub(super) fn request_headers_present(
    request_ctx: &ModelRequest,
    prompt_caching_enabled: bool,
) -> String {
    let mut headers = vec!["anthropic-version", "authorization", "content-type"];
    if prompt_caching_enabled {
        headers.push("anthropic-beta");
    }
    if request_ctx
        .aura_project_id
        .as_deref()
        .is_some_and(non_empty)
    {
        headers.push("X-Aura-Project-Id");
    }
    if request_ctx.aura_agent_id.as_deref().is_some_and(non_empty) {
        headers.push("X-Aura-Agent-Id");
    }
    if request_ctx
        .aura_session_id
        .as_deref()
        .is_some_and(non_empty)
    {
        headers.push("X-Aura-Session-Id");
    }
    if request_ctx.aura_org_id.as_deref().is_some_and(non_empty) {
        headers.push("X-Aura-Org-Id");
    }
    if request_ctx
        .upstream_provider_family
        .as_deref()
        .is_some_and(non_empty)
    {
        headers.push("X-Aura-Upstream-Provider-Family");
    }
    headers.join(",")
}

fn non_empty(value: &str) -> bool {
    !value.trim().is_empty()
}

/// Render `{"type":"auto"}` → `auto` for the visual block. Falls back
/// to the raw string when the shape is unfamiliar so we never lose
/// information.
pub(super) fn strip_tool_choice_braces(raw: &str) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(ty) = value.get("type").and_then(serde_json::Value::as_str) {
            return ty.to_string();
        }
    }
    raw.to_string()
}
