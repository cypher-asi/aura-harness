//! Choosing the provider and model an `aura -p` run uses.

use tracing::{info, warn};

use super::{HeadlessOptions, ToolFilter};
use crate::harness::build_provider;
use crate::model_selection::{may_be_alias, ModelSelection};
use crate::session_helpers;
use anyhow::Context;

/// The model a run uses, with the provider and live lists behind it.
pub(super) struct ModelChoice {
    pub(super) config: aura_agent::AgentLoopConfig,
    pub(super) selection: aura_model::ProviderSelection,
    pub(super) models: ModelSelection,
    pub(super) notes: Vec<String>,
}

/// Warn about `--allowed-tools`/`--disallowed-tools` names no tool has.
pub(super) fn warn_unknown_tools(tool_filter: &ToolFilter, tools: &[aura_model::ToolDefinition]) {
    let unknown = tool_filter.unknown(tools);
    if !unknown.is_empty() {
        let known: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        warn!(
            ?unknown,
            ?known,
            "Tool names in --allowed-tools/--disallowed-tools match no tool"
        );
    }
}

/// Pick the provider and the model for the run from `--provider` and
/// `--model`.
pub(super) async fn choose_model(args: &HeadlessOptions) -> anyhow::Result<ModelChoice> {
    let provider_name = args.provider.as_deref().unwrap_or("anthropic");
    let mock = provider_name == "mock";
    let mut config = if mock {
        session_helpers::default_agent_config_with_auth(aura_model::ENV_FALLBACK_MODEL, None)
    } else {
        session_helpers::default_agent_config(aura_model::ENV_FALLBACK_MODEL)
    };
    let (selection, auth) = if mock {
        // No sign-in state is needed, so tests and offline runs never
        // touch the keyring.
        (aura_model::mock_provider(), None)
    } else {
        let auth =
            std::sync::Arc::new(aura_auth::AuthManager::new().context("loading sign-in state")?);
        (build_provider(provider_name, &auth)?, Some(auth))
    };
    let summary = auth
        .as_ref()
        .map_or_else(aura_auth::AuthSummary::default, |auth| {
            crate::event_loop::auth::auth_summary(auth, config.auth_token.as_deref())
        });
    let mut models = ModelSelection::init(&mut config, summary);
    let notes = resolve_model(args, mock, auth.as_ref(), &mut config, &mut models).await?;
    Ok(ModelChoice {
        config,
        selection,
        models,
        notes,
    })
}

/// Apply `--model` (resolving a short name among the live lists) and
/// refresh what the chosen model needs; returns notes for the user.
async fn resolve_model(
    args: &HeadlessOptions,
    mock: bool,
    auth: Option<&std::sync::Arc<aura_auth::AuthManager>>,
    config: &mut aura_agent::AgentLoopConfig,
    models: &mut ModelSelection,
) -> anyhow::Result<Vec<String>> {
    let mut notes = Vec::new();
    // A short name (`opus`, `deepseek`) resolves among the listed models,
    // so load the live lists first. Mock runs skip this: they have no
    // sign-ins and must not touch the network.
    let mut local_probed = false;
    let mut plans_refreshed = false;
    if let (Some(model), Some(auth)) = (&args.model, auth) {
        if may_be_alias(model) {
            models.refresh_local().await;
            local_probed = true;
            if let Some(note) = models.refresh_plans(auth.as_ref()).await {
                warn!("{note}");
                notes.push(note);
            }
            plans_refreshed = true;
        }
    }
    if let Some(model) = &args.model {
        if let Err(problem) = models.use_for_run(config, model) {
            if !mock {
                anyhow::bail!("--model {model}: {problem}");
            }
        }
    }
    if let Some(auth) = auth {
        let local = |config: &aura_agent::AgentLoopConfig| {
            aura_model::model_catalog::local_model_name(&config.model).is_some()
        };
        if !local_probed && (models.is_gate() || local(config)) {
            models.refresh_local().await;
        }
        if models.is_gate() {
            if !plans_refreshed {
                if let Some(note) = models.refresh_plans(auth.as_ref()).await {
                    warn!("{note}");
                    notes.push(note);
                }
            }
            models.refresh_route(config);
            info!(model = %config.model, "{}", models.display());
        }
        if local(config) {
            if let Some(hint) = models.local_hint() {
                anyhow::bail!("{hint}");
            }
        }
    }
    Ok(notes)
}
