//! Output limits: the most tokens one reply of a model may hold.

use super::DEFAULT_LOCAL_MAX_OUTPUT_TOKENS;
use super::{backend_for, live_model_info, model_key, ModelBackend};

/// Output for a model whose API sets no limit of its own beyond its
/// context window (Grok, the open-weight models served by Fireworks):
/// the largest limit any API aura calls publishes. Fireworks lowers it to
/// what the window has left.
pub(super) const CONTEXT_BOUND_OUTPUT_TOKENS: u32 = 128_000;

/// Output for a model this table does not know.
pub(super) const UNKNOWN_OUTPUT_TOKENS: u32 = 32_000;

/// The most tokens one reply of `model` may hold: the limit its API
/// publishes, which requests are sent with unless a caller asks for less.
/// OpenRouter ids take the limit OpenRouter listed for them; other models
/// fall back to a listed limit only when the curated table does not know
/// them (see [the module docs](super#live-model-info)).
#[must_use]
pub fn max_output_tokens(model: &str) -> u32 {
    let backend = backend_for(model);
    if backend == ModelBackend::Local {
        return DEFAULT_LOCAL_MAX_OUTPUT_TOKENS;
    }
    let live = live_model_info(model).and_then(|info| info.max_output);
    if backend == ModelBackend::OpenRouter {
        if let Some(limit) = live {
            return limit;
        }
    }
    match curated_max_output_tokens(&model_key(model)) {
        UNKNOWN_OUTPUT_TOKENS => live.unwrap_or(UNKNOWN_OUTPUT_TOKENS),
        limit => limit,
    }
}

/// Output limit the curated table gives the model named by `key` (a
/// [`model_key`]), or [`UNKNOWN_OUTPUT_TOKENS`].
fn curated_max_output_tokens(key: &str) -> u32 {
    let has = |words: &[&str]| words.iter().any(|w| key.contains(w));
    if key.contains("claude") {
        return if has(&["opus-4-5", "sonnet-4-5", "haiku-4-5", "3-7-sonnet"]) {
            64_000
        } else if has(&["opus-4-1"]) || key.starts_with("claude-opus-4-20") {
            32_000
        } else if key.starts_with("claude-sonnet-4-20") {
            64_000
        } else if has(&["claude-3-5"]) {
            8_192
        } else if has(&["claude-3-"]) {
            4_096
        } else {
            128_000
        };
    }
    if has(&[
        "oss-120b", "qwen", "deepseek", "kimi", "minimax", "glm", "grok",
    ]) {
        CONTEXT_BOUND_OUTPUT_TOKENS
    } else if has(&["gpt-5", "gpt-6"]) {
        128_000
    } else if has(&["gpt-4-1"]) {
        32_768
    } else if has(&["gpt-4o"]) {
        16_384
    } else if key.starts_with("o1") || key.starts_with("o3") || key.starts_with("o4") {
        100_000
    } else if has(&["gemini"]) {
        65_536
    } else {
        UNKNOWN_OUTPUT_TOKENS
    }
}
