//! The `system` field: the system prompt block, an optional preamble block
//! and the cache salt.

use super::{cache_marker, COMPUTER_TOOL_NAME};
use crate::ModelRequest;

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
/// [`crates/surface/aura-node/src/session/state.rs`] `Session::new`), so this
/// guard is reached on real production traffic.
pub(in crate::anthropic) fn build_system_block(
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
            "cache_control": cache_marker()
        }]))
    } else {
        Some(serde_json::json!([{
            "type": "text",
            "text": system_prompt
        }]))
    }
}

/// [`build_system_block`] for `request`, with its `system_preamble` (if
/// any) as a separate first block. A `salt` (see [`system_salt`]) opens
/// the system text, after the preamble, which stays first.
pub(in crate::anthropic) fn build_request_system(
    request: &ModelRequest,
    prompt_caching_enabled: bool,
    salt: Option<&str>,
) -> Option<serde_json::Value> {
    let system_text = crate::cache_scope::opening(&request.system, salt);
    let system = build_system_block(&system_text, prompt_caching_enabled);
    let Some(preamble) = request
        .system_preamble
        .as_deref()
        .filter(|p| !p.trim().is_empty())
    else {
        return system;
    };
    let mut blocks = vec![serde_json::json!({"type": "text", "text": preamble})];
    if let Some(serde_json::Value::Array(rest)) = system {
        blocks.extend(rest);
    }
    Some(serde_json::Value::Array(blocks))
}

/// The salt the system text opens with, if any: `request`'s
/// [`ModelRequest::cache_salt`] when no tool carries it (see
/// [`super::convert_tool_entries_to_api`]), or always for an
/// `openai_family` upstream, which caches every prefix of the prompt from
/// its first token, wherever the router renders the tools.
pub(in crate::anthropic) fn system_salt(
    request: &ModelRequest,
    openai_family: bool,
) -> Option<&str> {
    let salt = request.cache_salt.as_deref()?;
    let tools_carry_it = request.tools.iter().any(|t| t.name != COMPUTER_TOOL_NAME);
    (openai_family || !tools_carry_it).then_some(salt)
}
