//! The model selector's catalog: local models, then the curated (or
//! live-listed) models of each cloud backend.

use super::{
    is_installed, local_model_id, local_model_name, ModelBackend, ModelOption, ModelTier,
    DEFAULT_LOCAL_MODEL,
};

/// Curated aura-router aliases offered in the selector.
const ROUTER_MODELS: &[(&str, &str, ModelTier)] = &[
    (
        "aura-claude-opus-4-7",
        "Claude Opus 4.7",
        ModelTier::Frontier,
    ),
    (
        "aura-claude-sonnet-4-6",
        "Claude Sonnet 4.6",
        ModelTier::Standard,
    ),
    ("aura-claude-haiku-4-5", "Claude Haiku 4.5", ModelTier::Fast),
    ("aura-gpt-5-4", "GPT-5.4", ModelTier::Frontier),
];

/// Curated models offered with a linked Claude plan.
const CLAUDE_MODELS: &[(&str, &str, ModelTier)] = &[
    (
        "claude/claude-opus-4-7",
        "Claude Opus 4.7",
        ModelTier::Frontier,
    ),
    (
        "claude/claude-sonnet-4-6",
        "Claude Sonnet 4.6",
        ModelTier::Standard,
    ),
    (
        "claude/claude-haiku-4-5",
        "Claude Haiku 4.5",
        ModelTier::Fast,
    ),
];

/// Curated models offered with a linked ChatGPT plan.
const CHATGPT_MODELS: &[(&str, &str, ModelTier)] =
    &[("chatgpt/gpt-5.4", "GPT-5.4", ModelTier::Frontier)];

/// Curated models offered through OpenRouter.
const OPENROUTER_MODELS: &[(&str, &str, ModelTier)] = &[
    (
        "openrouter/anthropic/claude-opus-4.7",
        "Claude Opus 4.7",
        ModelTier::Frontier,
    ),
    (
        "openrouter/anthropic/claude-sonnet-4.6",
        "Claude Sonnet 4.6",
        ModelTier::Standard,
    ),
    (
        "openrouter/anthropic/claude-haiku-4.5",
        "Claude Haiku 4.5",
        ModelTier::Fast,
    ),
    ("openrouter/openai/gpt-5.4", "GPT-5.4", ModelTier::Frontier),
];

/// Build the selector catalog: the default local model, any other
/// installed local models, the curated router aliases, then the
/// subscription models (shown regardless of link state; selection is
/// gated by [`ModelOption::required_provider`]).
#[must_use]
pub fn catalog(default_local: &str, installed_local: &[String]) -> Vec<ModelOption> {
    catalog_with_plans(default_local, installed_local, &[])
}

/// [`catalog`], with each curated list of a cloud backend (plans and
/// OpenRouter) replaced by `plan_models` (fetched live from the provider)
/// for every backend it covers.
#[must_use]
pub fn catalog_with_plans(
    default_local: &str,
    installed_local: &[String],
    plan_models: &[ModelOption],
) -> Vec<ModelOption> {
    let mut options = local_options(default_local, installed_local);
    for (models, backend) in [
        (ROUTER_MODELS, ModelBackend::Router),
        (CLAUDE_MODELS, ModelBackend::Claude),
        (CHATGPT_MODELS, ModelBackend::Chatgpt),
        (OPENROUTER_MODELS, ModelBackend::OpenRouter),
    ] {
        let live: Vec<&ModelOption> = plan_models
            .iter()
            .filter(|m| m.backend == backend)
            .collect();
        if !live.is_empty() {
            options.extend(live.into_iter().cloned());
            continue;
        }
        for (id, label, tier) in models {
            options.push(ModelOption {
                id: (*id).to_string(),
                label: (*label).to_string(),
                backend,
                installed: true,
                tier: *tier,
            });
        }
    }
    options
}

/// The default local model, then every other installed local model.
fn local_options(default_local: &str, installed_local: &[String]) -> Vec<ModelOption> {
    let mut options = Vec::new();
    let default_name = local_model_name(default_local).unwrap_or(DEFAULT_LOCAL_MODEL);
    options.push(ModelOption {
        id: local_model_id(default_name),
        label: default_name.to_string(),
        backend: ModelBackend::Local,
        installed: is_installed(installed_local, default_name),
        tier: ModelTier::Local,
    });
    for name in installed_local {
        if !is_installed(std::slice::from_ref(name), default_name) {
            options.push(ModelOption {
                id: local_model_id(name),
                label: name.clone(),
                backend: ModelBackend::Local,
                installed: true,
                tier: ModelTier::Local,
            });
        }
    }
    options
}
