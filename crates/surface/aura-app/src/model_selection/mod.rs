//! Terminal model selection: startup default, `/model` switching, and
//! persistence of the last choice.
//!
//! The default is `gate`: each turn goes to a model Gate picks from what
//! the user can run right now, a local model served by Ollama when
//! nothing is signed in. `/login` unlocks the aura-router cloud models and
//! `/login claude` / `/login chatgpt` unlock models billed to a linked
//! plan. `/login openrouter` unlocks the `openrouter/<vendor>/<model>`
//! models, billed to the user's OpenRouter credits. Every other model id
//! names its route. Gate's pick, and so who is billed, is shown at the
//! start of each turn and in the status line.

use std::path::PathBuf;
use std::time::Duration;

use aura_agent::AgentLoopConfig;
use aura_auth::{AuthSummary, CredentialResolver, ProviderKind};
use aura_gate::route::Candidate;
use aura_model::model_alias::{resolve_alias, resolve_exact, AliasCandidate};
use aura_model::model_catalog::{
    self, backend_for, infer_tier, local_model_name, LiveModelInfo, LivePrice, LocalServerStatus,
    ModelBackend, ModelOption, ModelTier, GATE_MODEL_ID, OPENROUTER_PREFIX,
};
use aura_model::ThinkingEffort;
use aura_ui::events::ModelOptionView;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use tracing::warn;

const LOCAL_STREAM_TIMEOUT: Duration = Duration::from_secs(600);

mod limits;
mod refresh;
mod saved;
mod select;
#[cfg(test)]
mod tests;

pub use limits::{routed_effort, ModelLimits};
use saved::*;

/// Persisted terminal preferences (`AURA_HOME/tui.json`).
#[derive(Debug, Default, Serialize, Deserialize)]
struct TuiPrefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    /// Wire name of the pinned [`ThinkingEffort`]; absent for `auto`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thinking: Option<String>,
}

/// Router-side agent-loop limits, captured before any local model is applied.
#[derive(Debug, Clone, Copy)]
struct RouterLimits {
    stream_timeout: Duration,
    stream_event_timeout: Duration,
}

/// Model selection state owned by the terminal event loop.
pub struct ModelSelection {
    current: String,
    default_local: String,
    local_url: String,
    local_status: LocalServerStatus,
    auth: AuthSummary,
    /// Models reported live by linked plans; empty means use the
    /// built-in lists.
    plan_models: Vec<ModelOption>,
    /// Models Gate stopped routing to after their route refused the
    /// user's sign-in or plan, until the sign-ins change.
    dropped: BTreeSet<String>,
    router_limits: RouterLimits,
    /// Thinking level pinned for every turn; `None` lets the agent loop
    /// pick one per model and step.
    thinking: Option<ThinkingEffort>,
    prefs_path: Option<PathBuf>,
}

impl ModelSelection {
    /// Resolve the startup model and thinking level and apply them to
    /// `config`.
    ///
    /// `auth` describes what the active identity can use.
    pub fn init(config: &mut AgentLoopConfig, auth: AuthSummary) -> Self {
        let prefs_path = prefs_path();
        if let Some(path) = &prefs_path {
            publish_saved_openrouter_info(path);
        }
        let prefs = prefs_path
            .as_deref()
            .and_then(load_prefs)
            .unwrap_or_default();
        let persisted = prefs.model;
        let env_model = std::env::var("AURA_DEFAULT_MODEL")
            .or_else(|_| std::env::var("AURA_ANTHROPIC_MODEL"))
            .ok();
        let default_local = model_catalog::default_local_model_id();
        let current = resolve_startup_model(env_model.as_deref(), persisted.as_deref(), |id| {
            usable(&auth, id)
        });
        let mut selection = Self {
            current: String::new(),
            default_local,
            local_url: model_catalog::local_base_url(),
            local_status: LocalServerStatus::Unreachable,
            auth,
            plan_models: Vec::new(),
            dropped: BTreeSet::new(),
            router_limits: RouterLimits {
                stream_timeout: config.stream_timeout,
                stream_event_timeout: config.stream_event_timeout,
            },
            thinking: prefs
                .thinking
                .as_deref()
                .and_then(ThinkingEffort::from_wire),
            prefs_path,
        };
        selection.apply(config, &current);
        config.user_thinking_effort = selection.thinking;
        selection
    }

    /// Selection fixed on `model`, with no sign-ins and no saved choice.
    #[cfg(test)]
    pub(crate) fn fixed(config: &AgentLoopConfig, model: &str) -> Self {
        Self::signed_in(config, model, AuthSummary::default())
    }

    /// [`Self::fixed`] with the sign-ins `auth`.
    #[cfg(test)]
    pub(crate) fn signed_in(config: &AgentLoopConfig, model: &str, auth: AuthSummary) -> Self {
        Self {
            current: model.to_string(),
            default_local: model_catalog::default_local_model_id(),
            local_url: model_catalog::local_base_url(),
            local_status: LocalServerStatus::Unreachable,
            auth,
            plan_models: Vec::new(),
            dropped: BTreeSet::new(),
            router_limits: RouterLimits {
                stream_timeout: config.stream_timeout,
                stream_event_timeout: config.stream_event_timeout,
            },
            thinking: None,
            prefs_path: None,
        }
    }

    /// Selected model id, [`GATE_MODEL_ID`] when Gate picks the model.
    pub fn current(&self) -> &str {
        &self.current
    }

    /// Whether Gate picks the model of each turn.
    pub fn is_gate(&self) -> bool {
        self.current == GATE_MODEL_ID
    }

    /// The models Gate may route to now, cheapest first.
    pub fn gate_candidates(&self) -> Vec<Candidate> {
        let catalog = model_catalog::catalog_with_plans(
            &self.default_local,
            self.installed_local(),
            &self.plan_models,
        );
        aura_gate::route::candidates(
            &catalog,
            |provider| self.auth.can_use(provider),
            |id| self.dropped.contains(id),
        )
    }

    /// Every model the user can run now on any route, cheapest first:
    /// unlike [`Self::gate_candidates`], each route of a model is kept and
    /// OpenRouter models without a curated price are too, so a benchmark
    /// arm can name any of them.
    pub fn runnable_models(&self) -> Vec<Candidate> {
        let catalog = model_catalog::catalog_with_plans(
            &self.default_local,
            self.installed_local(),
            &self.plan_models,
        );
        aura_gate::route::runnable(
            &catalog,
            |provider| self.auth.can_use(provider),
            |id| self.dropped.contains(id),
        )
    }

    /// Where Gate expects the next turn to go before its prompt is
    /// classified (its default for most categories), `None` when a model
    /// is pinned or nothing can run (then the turn goes to the default
    /// local model, whose setup hint explains what is missing). The
    /// status line shows it until a turn is routed.
    pub fn route(&self) -> Option<Candidate> {
        if !self.is_gate() {
            return None;
        }
        aura_gate::route::fixed_pick(&self.gate_candidates()).cloned()
    }

    /// The model that classifies prompts: a fast-tier model the user can
    /// run, a local one or one on a linked plan first (see
    /// [`aura_gate::judge::classify::classifier`]), else the local default.
    pub fn classifier_model(&self) -> String {
        aura_gate::judge::classify::classifier(&self.gate_candidates())
            .map_or_else(|| self.default_local.clone(), |c| c.id.clone())
    }

    /// The model the next turn runs on: the selected one, or Gate's pick.
    pub fn turn_model(&self) -> String {
        if !self.is_gate() {
            return self.current.clone();
        }
        self.route()
            .map_or_else(|| self.default_local.clone(), |pick| pick.id)
    }

    /// Point `config` at Gate's current pick. Returns `true` when the
    /// model changed.
    pub fn refresh_route(&self, config: &mut AgentLoopConfig) -> bool {
        let model = self.turn_model();
        if config.model == model {
            return false;
        }
        self.limits().configure(config, &model);
        true
    }

    /// Stop routing to `id` until the sign-ins change.
    pub fn drop_model(&mut self, id: &str) {
        self.dropped.insert(id.to_string());
    }

    /// Whether the user is logged in to zOS (and so aura-router).
    pub const fn logged_in(&self) -> bool {
        self.auth.zos_user.is_some()
    }

    /// Update what the active identity can use (after login, logout,
    /// link or unlink).
    pub fn set_auth(&mut self, auth: AuthSummary) {
        self.auth = auth;
        self.dropped.clear();
    }

    /// Whether `id` can run with the current sign-ins.
    pub fn usable(&self, id: &str) -> bool {
        usable(&self.auth, id)
    }
}

/// Startup model: explicit `AURA_DEFAULT_MODEL` wins, then the persisted
/// choice (unless it needs a sign-in the user doesn't have), then Gate.
fn resolve_startup_model(
    env_model: Option<&str>,
    persisted: Option<&str>,
    usable: impl Fn(&str) -> bool,
) -> String {
    if let Some(model) = env_model.map(str::trim).filter(|m| !m.is_empty()) {
        return model.to_string();
    }
    if let Some(model) = persisted.map(str::trim).filter(|m| !m.is_empty()) {
        if usable(model) {
            return model.to_string();
        }
    }
    GATE_MODEL_ID.to_string()
}

/// Whether `model`, as typed for `--model` or `/model`, may be a short
/// alias that [`ModelSelection::resolve`] matches against the listed
/// models. Not for `gate`, an explicit routed id (one with `/`), or a
/// full model name, which is used as given: one word starting with
/// `aura-`, `claude-` or `gpt-` (any case) and holding a digit
/// (`claude-opus-4-6`, `aura-claude-opus-4-7`, `gpt-5.4`). Callers load
/// the live model lists before resolving an alias.
pub fn may_be_alias(model: &str) -> bool {
    let model = model.trim();
    !model.is_empty()
        && !model.contains('/')
        && !model.eq_ignore_ascii_case(GATE_MODEL_ID)
        && !is_full_model_name(model)
}

/// Whether `model` is a full model name rather than an alias: see
/// [`may_be_alias`].
fn is_full_model_name(model: &str) -> bool {
    const FULL_NAME_PREFIXES: [&str; 3] = ["aura-", "claude-", "gpt-"];
    let has_prefix = FULL_NAME_PREFIXES.iter().any(|prefix| {
        model
            .get(..prefix.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
    });
    has_prefix && !model.contains(char::is_whitespace) && model.bytes().any(|b| b.is_ascii_digit())
}

fn usable(auth: &AuthSummary, id: &str) -> bool {
    id == GATE_MODEL_ID
        || backend_for(id)
            .required_provider()
            .is_none_or(|provider| auth.can_use(provider))
}

/// Name of the `gate` entry in `/model` and the status line.
const GATE_LABEL: &str = "Gate";

/// The name `/thinking` and the status line use for `effort`.
const fn thinking_name(effort: ThinkingEffort) -> &'static str {
    match effort.reasoning_effort_wire() {
        Some(name) => name,
        None => "off",
    }
}

fn unusable_reason(id: &str) -> String {
    match backend_for(id).required_provider() {
        Some(ProviderKind::Zos) | None => {
            format!("{id} is a cloud model — use /login first, or pick a local model")
        }
        Some(ProviderKind::OpenRouter) => format!(
            "{id} runs on your OpenRouter account — use /login openrouter first, or pick \
             another model"
        ),
        Some(provider) => format!(
            "{id} runs on your {} plan — use /login {} first, or pick another model",
            provider.label(),
            provider.id()
        ),
    }
}

/// Selector heading for the model `id` served by `backend`; OpenRouter
/// models are grouped by vendor (`OpenRouter · anthropic`).
fn group_heading(backend: ModelBackend, id: &str) -> String {
    match backend {
        ModelBackend::Local => "Local (no login)".to_string(),
        ModelBackend::Router => "Cloud (aura-router)".to_string(),
        ModelBackend::Claude => "Claude plan (direct)".to_string(),
        ModelBackend::Chatgpt => "ChatGPT plan (direct)".to_string(),
        ModelBackend::OpenRouter => {
            let vendor = id
                .strip_prefix(OPENROUTER_PREFIX)
                .and_then(|rest| rest.split('/').next())
                .filter(|vendor| !vendor.is_empty());
            match vendor {
                Some(vendor) => format!("OpenRouter · {vendor}"),
                None => "OpenRouter".to_string(),
            }
        }
    }
}

/// How a note describes the list shown after a listing failed.
const fn shown_list(nothing_saved: bool) -> &'static str {
    if nothing_saved {
        "showing the built-in list"
    } else {
        "showing the list saved when it last loaded"
    }
}
