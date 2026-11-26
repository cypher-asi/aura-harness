//! Terminal preferences and the model lists saved beside them.

use super::*;

pub(super) fn prefs_path() -> Option<PathBuf> {
    aura_config::AuraHome::resolve()
        .ok()
        .map(|home| home.path.join("tui.json"))
}

pub(super) fn load_prefs(path: &std::path::Path) -> Option<TuiPrefs> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Make the limits and prices of OpenRouter's `entries` what the catalog
/// knows live about OpenRouter models.
pub(super) fn publish_openrouter_info(entries: &[(ModelOption, LiveModelInfo)]) {
    model_catalog::replace_live_models(
        ModelBackend::OpenRouter,
        entries
            .iter()
            .map(|(option, info)| (option.id.clone(), *info)),
    );
}

/// Publish the limits and prices of the OpenRouter listing saved beside
/// `prefs` (`tui.json`), so OpenRouter calls are priced from the start:
/// a fresh listing is fetched only when the model lists are refreshed,
/// and a run on a model named in full never refreshes them. Nothing is
/// published when no list is saved there.
pub(super) fn publish_saved_openrouter_info(prefs: &std::path::Path) {
    let saved = load_openrouter_models(&prefs.with_file_name(OPENROUTER_MODELS_FILE));
    if !saved.is_empty() {
        publish_openrouter_info(&saved);
    }
}

/// Beside `tui.json`: the Claude plan's model list as it last loaded,
/// used when it can't be loaded. The built-in list is older and would
/// leave Gate routing among models the plan has since replaced.
pub(super) const PLAN_MODELS_FILE: &str = "plan_models.json";

/// A plan model as saved in [`PLAN_MODELS_FILE`].
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SavedPlanModel {
    id: String,
    label: String,
}

pub(super) fn save_plan_models(path: &std::path::Path, models: &[ModelOption]) {
    let saved: Vec<SavedPlanModel> = models
        .iter()
        .map(|m| SavedPlanModel {
            id: m.id.clone(),
            label: m.label.clone(),
        })
        .collect();
    let written = serde_json::to_vec_pretty(&saved)
        .map_err(std::io::Error::other)
        .and_then(|bytes| std::fs::write(path, bytes));
    if let Err(e) = written {
        warn!(path = %path.display(), error = %e, "Could not save the plan's model list");
    }
}

/// The Claude plan models saved at `path`; none when there is no
/// readable list.
pub(super) fn load_plan_models(path: &std::path::Path) -> Vec<ModelOption> {
    let saved: Vec<SavedPlanModel> = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    saved
        .into_iter()
        .filter(|m| backend_for(&m.id) == ModelBackend::Claude)
        .map(|m| ModelOption {
            tier: infer_tier(&m.id),
            backend: ModelBackend::Claude,
            installed: true,
            id: m.id,
            label: m.label,
        })
        .collect()
}

/// Beside `tui.json`: the OpenRouter account's model list as it last
/// loaded, with the limits and prices it gave, used when it can't be
/// loaded.
pub(super) const OPENROUTER_MODELS_FILE: &str = "openrouter_models.json";

/// An OpenRouter model as saved in [`OPENROUTER_MODELS_FILE`].
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SavedOpenRouterModel {
    id: String,
    label: String,
    #[serde(default)]
    context_window: Option<u64>,
    #[serde(default)]
    max_output: Option<u32>,
    #[serde(default)]
    price: Option<SavedPrice>,
}

/// A [`LivePrice`] as saved, in USD per million tokens.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SavedPrice {
    input: f64,
    output: f64,
    cache_write: f64,
    cache_read: f64,
}

pub(super) fn save_openrouter_models(
    path: &std::path::Path,
    entries: &[(ModelOption, LiveModelInfo)],
) {
    let saved: Vec<SavedOpenRouterModel> = entries
        .iter()
        .map(|(option, info)| SavedOpenRouterModel {
            id: option.id.clone(),
            label: option.label.clone(),
            context_window: info.context_window,
            max_output: info.max_output,
            price: info.price.map(|p| SavedPrice {
                input: p.input,
                output: p.output,
                cache_write: p.cache_write,
                cache_read: p.cache_read,
            }),
        })
        .collect();
    let written = serde_json::to_vec_pretty(&saved)
        .map_err(std::io::Error::other)
        .and_then(|bytes| std::fs::write(path, bytes));
    if let Err(e) = written {
        warn!(path = %path.display(), error = %e, "Could not save the OpenRouter model list");
    }
}

/// The OpenRouter models saved at `path`; none when there is no readable
/// list.
pub(super) fn load_openrouter_models(path: &std::path::Path) -> Vec<(ModelOption, LiveModelInfo)> {
    let saved: Vec<SavedOpenRouterModel> = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    saved
        .into_iter()
        .filter(|m| backend_for(&m.id) == ModelBackend::OpenRouter)
        .map(|m| {
            let info = LiveModelInfo {
                context_window: m.context_window,
                max_output: m.max_output,
                price: m.price.map(|p| LivePrice {
                    input: p.input,
                    output: p.output,
                    cache_write: p.cache_write,
                    cache_read: p.cache_read,
                }),
            };
            let option = ModelOption {
                tier: infer_tier(&m.id),
                backend: ModelBackend::OpenRouter,
                installed: true,
                id: m.id,
                label: m.label,
            };
            (option, info)
        })
        .collect()
}
