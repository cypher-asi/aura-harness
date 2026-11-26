//! What a model can do with thinking effort, prompt caching and tool choice.

use super::{is_snapshot_of, model_key};

/// Whether changing the thinking effort between two requests of one
/// conversation makes `model`'s provider write the conversation to its
/// prompt cache again. Claude models do: only the tools and the system
/// prompt are still read from the cache after a change.
#[must_use]
pub fn effort_change_resets_prompt_cache(model: &str) -> bool {
    model_key(model).starts_with("claude-")
}

/// Whether an agent's turns on `model` reason at high effort when nobody
/// chose one: Claude Sonnet 5.5 and the Claude Opus 5 models. They get
/// the work that is hard to get right, where an attempt that fails costs
/// more than the extra reasoning does. Not max: at max one call on a long
/// conversation thinks for minutes, and a stream cut off before it ends
/// starts that thinking over.
#[must_use]
pub fn reasons_high(model: &str) -> bool {
    let key = model_key(model);
    is_snapshot_of(&key, "claude-sonnet-5-5")
        || is_snapshot_of(&key, "claude-opus-5")
        || is_snapshot_of(&key, "claude-opus-5-5")
}

/// Whether `model` reasons harder at max effort than at high: the Claude
/// models whose effort ladder goes up to `max` (Sonnet and Opus 4.6 and
/// later, Fable and Mythos 5). Others fold max into their highest effort.
#[must_use]
pub fn has_max_effort(model: &str) -> bool {
    let key = model_key(model);
    [
        "claude-opus-4-6",
        "claude-sonnet-4-6",
        "claude-opus-4-7",
        "claude-opus-4-8",
        "claude-opus-5",
        "claude-sonnet-5",
        "claude-fable-5",
        "claude-mythos-5",
    ]
    .iter()
    .any(|family| key.starts_with(family))
}

/// Whether `model` takes a tool choice that forces a call (any tool, or a
/// named one). Claude Sonnet 5.5 does not: the API answers `tool_choice:
/// type "tool" and "any" are not supported for this model`. Nor does Opus
/// 5.5, which cannot turn thinking off, and forced tool use needs thinking
/// off. Ask them with `auto` instead.
#[must_use]
pub fn takes_forced_tool_choice(model: &str) -> bool {
    let key = model_key(model);
    !["claude-sonnet-5-5", "claude-opus-5-5"]
        .iter()
        .any(|name| is_snapshot_of(&key, name))
}
