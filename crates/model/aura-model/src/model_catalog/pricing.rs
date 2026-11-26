//! List prices, how a call's tokens are billed, and which model
//! supersedes which.

use super::{backend_for, is_snapshot_of, live_model_info, model_key, ModelBackend};
use crate::Usage;

/// API list price of a model, in USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Price {
    input: f64,
    output: f64,
    cache_write: f64,
    cache_read: f64,
}

const fn price(input: f64, output: f64, cache_write: f64, cache_read: f64) -> Price {
    Price {
        input,
        output,
        cache_write,
        cache_read,
    }
}

/// Standard short-context list prices (Anthropic and OpenAI, November
/// 2026). Keys write `.` as `-`; a key also covers its dated snapshots.
/// Anthropic cache writes are the 5-minute rate.
const PRICES: &[(&str, Price)] = &[
    ("claude-fable-5-1", price(10.0, 50.0, 12.5, 0.25)),
    ("claude-fable-5", price(10.0, 50.0, 12.5, 1.0)),
    ("claude-mythos-5-1", price(10.0, 50.0, 12.5, 0.25)),
    ("claude-mythos-5", price(10.0, 50.0, 12.5, 1.0)),
    ("claude-opus-5-5", price(4.0, 20.0, 5.0, 0.2)),
    ("claude-opus-5", price(5.0, 25.0, 6.25, 0.5)),
    ("claude-opus-4-8", price(5.0, 25.0, 6.25, 0.5)),
    ("claude-opus-4-7", price(5.0, 25.0, 6.25, 0.5)),
    ("claude-opus-4-6", price(5.0, 25.0, 6.25, 0.5)),
    ("claude-opus-4-5", price(5.0, 25.0, 6.25, 0.5)),
    ("claude-opus-4-1", price(15.0, 75.0, 18.75, 1.5)),
    ("claude-opus-4", price(15.0, 75.0, 18.75, 1.5)),
    ("claude-sonnet-5-5", price(2.0, 10.0, 2.5, 0.1)),
    ("claude-sonnet-5", price(2.0, 10.0, 2.5, 0.2)),
    ("claude-sonnet-4-6", price(3.0, 15.0, 3.75, 0.3)),
    ("claude-sonnet-4-5", price(3.0, 15.0, 3.75, 0.3)),
    ("claude-sonnet-4", price(3.0, 15.0, 3.75, 0.3)),
    ("claude-haiku-5-5", price(0.1, 0.5, 0.125, 0.01)),
    ("claude-haiku-4-5", price(1.0, 5.0, 1.25, 0.1)),
    ("claude-3-5-haiku", price(0.8, 4.0, 1.0, 0.08)),
    ("gpt-6-astra", price(10.0, 50.0, 12.5, 1.0)),
    ("gpt-6-1-sol", price(2.0, 10.0, 2.5, 0.1)),
    ("gpt-6-1-terra", price(1.0, 5.0, 1.25, 0.05)),
    ("gpt-6-sol", price(2.0, 10.0, 2.5, 0.2)),
    ("gpt-6-luna", price(0.1, 0.5, 0.125, 0.01)),
    ("gpt-5-6-sol", price(4.0, 20.0, 5.0, 0.4)),
    ("gpt-5-6-terra", price(2.0, 12.0, 2.5, 0.2)),
    ("gpt-5-6-luna", price(0.2, 1.2, 0.25, 0.02)),
    ("gpt-5-5-pro", price(30.0, 180.0, 30.0, 30.0)),
    ("gpt-5-5", price(5.0, 30.0, 5.0, 0.5)),
    ("gpt-5-4-pro", price(30.0, 180.0, 30.0, 30.0)),
    ("gpt-5-4-mini", price(0.75, 4.5, 0.75, 0.075)),
    ("gpt-5-4-nano", price(0.2, 1.25, 0.2, 0.02)),
    ("gpt-5-4", price(2.5, 15.0, 2.5, 0.25)),
    ("gpt-5-2-pro", price(21.0, 168.0, 21.0, 21.0)),
    ("gpt-5-2", price(1.75, 14.0, 1.75, 0.175)),
    ("gpt-5-1", price(1.25, 10.0, 1.25, 0.125)),
    ("gpt-5-pro", price(15.0, 120.0, 15.0, 15.0)),
    ("gpt-5-mini", price(0.25, 2.0, 0.25, 0.025)),
    ("gpt-5-nano", price(0.05, 0.4, 0.05, 0.005)),
    ("gpt-5", price(1.25, 10.0, 1.25, 0.125)),
];

/// List price of `model`: the curated [`PRICES`] entry, else the price
/// its provider listed (see [the module docs](super#live-model-info)).
fn list_price(model: &str) -> Option<Price> {
    if backend_for(model) == ModelBackend::Local {
        return None;
    }
    let key = model_key(model);
    PRICES
        .iter()
        .find(|(name, _)| is_snapshot_of(&key, name))
        .map(|(_, price)| *price)
        .or_else(|| {
            live_model_info(model)
                .and_then(|info| info.price)
                .map(|live| price(live.input, live.output, live.cache_write, live.cache_read))
        })
}

/// The family `key` (a [`model_key`]) belongs to and its version there:
/// the words of its name, and the numbers among them
/// (`claude-sonnet-5-5` -> `claude-sonnet` 5.5, `gpt-5-4-mini` ->
/// `gpt-mini` 5.4). A dated snapshot's date is left out. `None` when the
/// name has no version.
fn family_version(key: &str) -> Option<(String, Vec<u32>)> {
    let mut words = Vec::new();
    let mut version = Vec::new();
    for part in key.split('-') {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            words.push(part);
        } else if part.len() >= 4 {
            break;
        } else {
            version.push(part.parse().ok()?);
        }
    }
    (!version.is_empty()).then(|| (words.join("-"), version))
}

/// Whether `newer` leaves no reason to send work to `older`: it is a
/// later version of the same model family, and no kind of token costs
/// more on it. A model whose name has no version or whose price is not
/// known supersedes nothing, and nothing supersedes it.
#[must_use]
pub fn supersedes(newer: &str, older: &str) -> bool {
    let (Some(new), Some(old)) = (list_price(newer), list_price(older)) else {
        return false;
    };
    let (Some((new_family, new_version)), Some((old_family, old_version))) = (
        family_version(&model_key(newer)),
        family_version(&model_key(older)),
    ) else {
        return false;
    };
    new_family == old_family
        && new_version > old_version
        && new.input <= old.input
        && new.output <= old.output
        && new.cache_write <= old.cache_write
        && new.cache_read <= old.cache_read
}

/// A call's prompt and output tokens, split the way they are billed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BilledTokens {
    /// Prompt tokens neither read from nor written to the cache
    pub uncached_input: u64,
    /// Prompt tokens written to the cache
    pub cache_write: u64,
    /// Prompt tokens read from the cache
    pub cache_read: u64,
    /// Output tokens
    pub output: u64,
}

/// Split `usage` of a call to `model` the way it is billed.
#[must_use]
pub fn billed_tokens(model: &str, usage: &Usage) -> BilledTokens {
    let cache_read = usage.cache_read_input_tokens.unwrap_or(0);
    let cache_write = usage.cache_creation_input_tokens.unwrap_or(0);
    // OpenAI counts cached tokens inside `input_tokens`; Anthropic-format
    // responses report them on top of it. Only the ChatGPT route and
    // aura-router's GPT aliases answer in OpenAI shape: OpenRouter's
    // `/v1/messages` (and the local server's) reports Anthropic-format
    // usage whatever model is upstream. Pass the id the call asked for:
    // the name an answer comes back with does not say which endpoint
    // reported it.
    let openai = match backend_for(model) {
        ModelBackend::Chatgpt => true,
        ModelBackend::Router => model_key(model).starts_with("gpt-"),
        ModelBackend::Local | ModelBackend::Claude | ModelBackend::OpenRouter => false,
    };
    let uncached_input = if openai {
        usage.input_tokens.saturating_sub(cache_read)
    } else {
        usage.input_tokens
    };
    BilledTokens {
        uncached_input,
        cache_write,
        cache_read,
        output: usage.output_tokens,
    }
}

/// Tokens a model call used: its whole prompt (cached or not) plus output.
#[must_use]
pub fn call_tokens(model: &str, usage: &Usage) -> u64 {
    let tokens = billed_tokens(model, usage);
    tokens.uncached_input + tokens.cache_write + tokens.cache_read + tokens.output
}

/// What a model call costs at API list prices, in USD. `None` for local
/// models and models without a known price.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn estimate_cost_usd(model: &str, usage: &Usage) -> Option<f64> {
    let price = list_price(model)?;
    let tokens = billed_tokens(model, usage);
    let micro_usd = tokens.uncached_input as f64 * price.input
        + tokens.cache_write as f64 * price.cache_write
        + tokens.cache_read as f64 * price.cache_read
        + tokens.output as f64 * price.output;
    Some(micro_usd / 1_000_000.0)
}
