//! Model info listed live by providers (see [the module
//! docs](super#live-model-info) for its invariants).

use std::collections::BTreeMap;
use std::sync::{PoisonError, RwLock};

use super::{backend_for, without_variant, ModelBackend};

/// What a provider's model listing says about one model. Every field is
/// optional: a listing may leave any of them out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiveModelInfo {
    /// Context window, in tokens.
    pub context_window: Option<u64>,
    /// Most output tokens one reply may hold.
    pub max_output: Option<u32>,
    /// Price per token kind.
    pub price: Option<LivePrice>,
}

/// A listed model price, in USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LivePrice {
    /// Uncached prompt tokens.
    pub input: f64,
    /// Output tokens.
    pub output: f64,
    /// Prompt tokens written to the cache.
    pub cache_write: f64,
    /// Prompt tokens read from the cache.
    pub cache_read: f64,
}

impl LivePrice {
    /// Whether every rate is a real, non-negative number.
    fn is_valid(&self) -> bool {
        [self.input, self.output, self.cache_write, self.cache_read]
            .iter()
            .all(|rate| rate.is_finite() && *rate >= 0.0)
    }
}

impl LiveModelInfo {
    /// `self` without the values no model can have (zero limits, prices
    /// that are negative or not numbers), or `None` when nothing is left.
    fn sanitized(self) -> Option<Self> {
        let info = Self {
            context_window: self.context_window.filter(|tokens| *tokens > 0),
            max_output: self.max_output.filter(|tokens| *tokens > 0),
            price: self.price.filter(LivePrice::is_valid),
        };
        let is_empty =
            info.context_window.is_none() && info.max_output.is_none() && info.price.is_none();
        (!is_empty).then_some(info)
    }
}

/// Listed model info by full model id; see [the module
/// docs](super#live-model-info) for who writes it and how it is read.
static LIVE_MODELS: RwLock<BTreeMap<String, LiveModelInfo>> = RwLock::new(BTreeMap::new());

/// Replace everything known live about `backend`'s models with `entries`
/// (full model id, info), as fetched from its provider's model listing.
/// Entries whose id names another backend are ignored, so each backend's
/// entries always come from its own latest listing; values no model can
/// have are dropped.
pub fn replace_live_models(
    backend: ModelBackend,
    entries: impl IntoIterator<Item = (String, LiveModelInfo)>,
) {
    let fresh: Vec<(String, LiveModelInfo)> = entries
        .into_iter()
        .filter(|(id, _)| backend_for(id) == backend)
        .filter_map(|(id, info)| info.sanitized().map(|info| (id, info)))
        .collect();
    let mut table = LIVE_MODELS.write().unwrap_or_else(PoisonError::into_inner);
    table.retain(|id, _| backend_for(id) != backend);
    table.extend(fresh);
}

/// What the latest model listing of its provider said about `model`:
/// the entry listed under `model` itself, else, for an OpenRouter id,
/// under that id without its `:variant` (`:nitro` is listed as its base
/// model).
#[must_use]
pub fn live_model_info(model: &str) -> Option<LiveModelInfo> {
    let table = LIVE_MODELS.read().unwrap_or_else(PoisonError::into_inner);
    let listed = |id: &str| table.get(id).copied();
    listed(model).or_else(|| {
        if backend_for(model) != ModelBackend::OpenRouter {
            return None;
        }
        listed(without_variant(model)?)
    })
}

/// Context window the latest model listing gave `model`, in tokens.
#[must_use]
pub fn live_context_window(model: &str) -> Option<u64> {
    live_model_info(model).and_then(|info| info.context_window)
}
