//! Conversion between Aura's request/response types and the Anthropic
//! Messages API shapes: thinking and effort settings, the system block,
//! messages and their tool-result pairing, and the tool list.

mod messages;
mod system;
mod thinking;
mod tool_pairing;
mod tools;

pub(super) use messages::{convert_messages_to_api, convert_response_to_aura};
pub(super) use system::{build_request_system, system_salt};
pub(super) use thinking::{resolve_output_config, resolve_thinking};
pub(super) use tool_pairing::{
    dedupe_tool_results, drop_misplaced_tool_results, inject_missing_tool_results,
};
pub(super) use tools::{
    convert_tool_entries_to_api, request_uses_computer_tool, tool_choice_for, COMPUTER_TOOL_NAME,
    COMPUTER_USE_BETA,
};

// Steps the provider reaches only through the functions above; the
// anthropic tests check them one by one.
#[cfg(test)]
pub(super) use messages::api_tool_id;
#[cfg(test)]
pub(super) use system::build_system_block;
#[cfg(test)]
pub(super) use tools::{convert_tool_choice, convert_tools_to_api};

/// Beta header token that enables the one-hour cache lifetime.
pub(super) const EXTENDED_CACHE_TTL_BETA: &str = "extended-cache-ttl-2025-04-11";

/// Whether `AURA_PROMPT_CACHE_TTL=1h` asks for one-hour cache entries
/// instead of the default five minutes. Read once: every breakpoint in a
/// request must carry the same TTL, and it must not change between
/// requests or the cached prefixes stop matching.
pub(super) fn extended_cache_ttl() -> bool {
    static EXTENDED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *EXTENDED.get_or_init(|| {
        std::env::var("AURA_PROMPT_CACHE_TTL")
            .is_ok_and(|value| value.trim().eq_ignore_ascii_case("1h"))
    })
}

/// `cache_control` value for a prompt-cache breakpoint.
pub(super) fn cache_marker() -> serde_json::Value {
    if extended_cache_ttl() {
        serde_json::json!({"type": "ephemeral", "ttl": "1h"})
    } else {
        serde_json::json!({"type": "ephemeral"})
    }
}

/// The model name the capability checks (thinking mode, effort ladder,
/// forced tool choice, Anthropic proxy features) know `model` by. A
/// gateway slug such as OpenRouter's `anthropic/claude-sonnet-4.6:nitro`
/// becomes `claude-sonnet-4.6`; any other name is returned as it is. The
/// request itself still names the model the way its upstream wants.
pub(super) fn capability_model(model: &str) -> &str {
    let model = model.trim();
    let Some((_, name)) = model.split_once('/') else {
        return model;
    };
    let name = name.split_once(':').map_or(name, |(name, _)| name);
    if name.trim().is_empty() {
        model
    } else {
        name
    }
}
