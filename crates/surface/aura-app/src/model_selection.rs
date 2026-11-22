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
use aura_agent_gate::Candidate;
use aura_auth::{AuthSummary, CredentialResolver, ProviderKind};
use aura_model::model_catalog::{
    self, backend_for, infer_tier, local_model_name, LiveModelInfo, LivePrice, LocalServerStatus,
    ModelBackend, ModelOption, GATE_MODEL_ID, OPENROUTER_PREFIX,
};
use aura_model::ThinkingEffort;
use aura_ui_terminal::events::ModelOptionView;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use tracing::warn;

const LOCAL_STREAM_TIMEOUT: Duration = Duration::from_secs(600);

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
        aura_agent_gate::candidates(
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
        aura_agent_gate::fixed_pick(&self.gate_candidates()).cloned()
    }

    /// The model that classifies prompts: the cheapest fast-tier model the
    /// user can run, else the local default.
    pub fn classifier_model(&self) -> String {
        aura_agent_gate::classify::classifier(&self.gate_candidates())
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

    /// Reload the model lists of the linked Claude plan and OpenRouter
    /// account, concurrently. Returns a user-facing note (one line per
    /// provider) when a linked provider's list could not be loaded; the
    /// list saved when it last loaded is used then, else the built-in one.
    pub async fn refresh_plans(&mut self, resolver: &dyn CredentialResolver) -> Option<String> {
        let claude = self.auth.can_use(ProviderKind::Claude);
        let openrouter = self.auth.can_use(ProviderKind::OpenRouter);
        let (claude_listed, openrouter_listed) = tokio::join!(
            async {
                if claude {
                    let listed = aura_model::plan_models::fetch_claude_models(resolver).await;
                    Some(listed.map_err(|e| e.to_string()))
                } else {
                    None
                }
            },
            async {
                if openrouter {
                    let listed = aura_model::plan_models::fetch_openrouter_models(resolver).await;
                    Some(listed.map_err(|e| e.to_string()))
                } else {
                    None
                }
            },
        );
        let claude_note = match claude_listed {
            Some(listed) => self.plans_listed(listed),
            None => {
                self.set_backend_models(ModelBackend::Claude, Vec::new());
                None
            }
        };
        let openrouter_note = match openrouter_listed {
            Some(listed) => self.openrouter_listed(listed),
            None => {
                self.set_backend_models(ModelBackend::OpenRouter, Vec::new());
                None
            }
        };
        let notes: Vec<String> = [claude_note, openrouter_note]
            .into_iter()
            .flatten()
            .collect();
        (!notes.is_empty()).then(|| notes.join("\n"))
    }

    /// Replace the live models of `backend` with `models`; an empty list
    /// means the built-in one.
    fn set_backend_models(&mut self, backend: ModelBackend, models: Vec<ModelOption>) {
        self.plan_models.retain(|m| m.backend != backend);
        self.plan_models.extend(models);
    }

    /// Path beside `tui.json` named `file`, `None` without a prefs path.
    fn saved_path(&self, file: &str) -> Option<PathBuf> {
        self.prefs_path
            .as_ref()
            .map(|prefs| prefs.with_file_name(file))
    }

    /// Take in the Claude plan's model list as `listed` just now, or why
    /// it could not be. Returns the Claude note of [`Self::refresh_plans`].
    fn plans_listed(&mut self, listed: Result<Vec<ModelOption>, String>) -> Option<String> {
        let saved_at = self.saved_path(PLAN_MODELS_FILE);
        match listed {
            Ok(models) => {
                if let Some(path) = &saved_at {
                    save_plan_models(path, &models);
                }
                self.set_backend_models(ModelBackend::Claude, models);
                None
            }
            Err(problem) => {
                warn!(error = %problem, "Failed to list Claude plan models");
                let saved = saved_at
                    .as_deref()
                    .map(load_plan_models)
                    .unwrap_or_default();
                let shown = shown_list(saved.is_empty());
                self.set_backend_models(ModelBackend::Claude, saved);
                Some(format!(
                    "Couldn't load your Claude plan's model list ({problem}); {shown}. Any \
                     model works with /model claude/<model-id>."
                ))
            }
        }
    }

    /// Take in the OpenRouter account's model list as `listed` just now,
    /// or why it could not be, and publish the limits and prices it gives.
    /// Returns the OpenRouter note of [`Self::refresh_plans`].
    fn openrouter_listed(
        &mut self,
        listed: Result<Vec<(ModelOption, LiveModelInfo)>, String>,
    ) -> Option<String> {
        let saved_at = self.saved_path(OPENROUTER_MODELS_FILE);
        let (entries, note) = match listed {
            Ok(entries) => {
                if let Some(path) = &saved_at {
                    save_openrouter_models(path, &entries);
                }
                (entries, None)
            }
            Err(problem) => {
                warn!(error = %problem, "Failed to list OpenRouter models");
                let saved = saved_at
                    .as_deref()
                    .map(load_openrouter_models)
                    .unwrap_or_default();
                let shown = shown_list(saved.is_empty());
                let note = format!(
                    "Couldn't load your OpenRouter account's model list ({problem}); {shown}. \
                     Any model works with /model openrouter/<vendor>/<model>."
                );
                (saved, Some(note))
            }
        };
        if note.is_none() || !entries.is_empty() {
            model_catalog::replace_live_models(
                ModelBackend::OpenRouter,
                entries
                    .iter()
                    .map(|(option, info)| (option.id.clone(), *info)),
            );
        }
        let models = entries.into_iter().map(|(option, _)| option).collect();
        self.set_backend_models(ModelBackend::OpenRouter, models);
        note
    }

    /// Re-probe the local model server.
    pub async fn refresh_local(&mut self) {
        self.local_status = model_catalog::probe_local_server(&self.local_url).await;
    }

    fn installed_local(&self) -> &[String] {
        match &self.local_status {
            LocalServerStatus::Ready { models, .. } => models,
            _ => &[],
        }
    }

    /// Selector entries, always including the current model.
    pub fn options(&self) -> Vec<ModelOptionView> {
        let mut options = model_catalog::catalog_with_plans(
            &self.default_local,
            self.installed_local(),
            &self.plan_models,
        );
        if !options.iter().any(|o| o.id == self.current) {
            let backend = backend_for(&self.current);
            options.push(ModelOption {
                id: self.current.clone(),
                label: local_model_name(&self.current)
                    .unwrap_or(&self.current)
                    .to_string(),
                backend,
                installed: backend != ModelBackend::Local || self.local_installed(&self.current),
                tier: infer_tier(&self.current),
            });
        }
        let gate = ModelOptionView {
            id: GATE_MODEL_ID.to_string(),
            label: GATE_LABEL.to_string(),
            local: false,
            installed: true,
            group: GATE_LABEL.to_string(),
            provider: None,
            available: true,
            context_window: None,
        };
        std::iter::once(gate)
            .chain(options.into_iter().map(|o| ModelOptionView {
                local: o.backend == ModelBackend::Local,
                group: group_heading(o.backend, &o.id),
                provider: o.required_provider().map(|p| p.id().to_string()),
                available: self.usable(&o.id),
                context_window: Some(aura_engine::model_context::context_window_for_model(&o.id)),
                id: o.id,
                label: o.label,
                installed: o.installed,
            }))
            .collect()
    }

    /// `UiCommand::SetModels` payload for the current state.
    pub fn ui_command(&self) -> aura_ui_terminal::UiCommand {
        aura_ui_terminal::UiCommand::SetModels {
            options: self.options(),
            current: self.current.clone(),
            route: self
                .is_gate()
                .then(|| model_catalog::route_label(&self.turn_model())),
            user: self.auth.zos_user.clone(),
            plans: self.auth.plan_names.clone(),
            thinking: self
                .thinking
                .map(|effort| thinking_name(effort).to_string()),
        }
    }

    /// Pin the thinking level of every turn to the one named `level`
    /// (`high`, `off`, ...), or let each model pick its own for `None`.
    /// Returns a user-facing problem for a name that is not a level.
    pub fn set_thinking(
        &mut self,
        config: &mut AgentLoopConfig,
        level: Option<&str>,
    ) -> Result<(), String> {
        let effort = match level {
            None => None,
            Some(name) => Some(
                ThinkingEffort::from_wire(name)
                    .ok_or_else(|| format!("Unknown thinking level: {name}"))?,
            ),
        };
        self.thinking = effort;
        config.user_thinking_effort = effort;
        self.persist();
        Ok(())
    }

    /// The pinned thinking level, `auto` when each model picks its own.
    pub fn thinking_display(&self) -> &'static str {
        self.thinking.map_or("auto", thinking_name)
    }

    fn local_installed(&self, id: &str) -> bool {
        local_model_name(id)
            .is_some_and(|name| model_catalog::is_installed(self.installed_local(), name))
    }

    /// Switch to `id`. Returns a user-facing problem when the switch is refused.
    pub fn select(&mut self, config: &mut AgentLoopConfig, id: &str) -> Result<(), String> {
        self.use_for_run(config, id)?;
        self.persist();
        Ok(())
    }

    /// Switch to `id` for this process only, leaving the saved choice
    /// alone. Returns a user-facing problem when the switch is refused.
    pub fn use_for_run(&mut self, config: &mut AgentLoopConfig, id: &str) -> Result<(), String> {
        let id = id.trim();
        if id.is_empty() {
            return Err("Model id is empty".to_string());
        }
        if !self.usable(id) {
            return Err(unusable_reason(id));
        }
        self.apply(config, id);
        Ok(())
    }

    /// Switch to Gate when the current model needs a sign-in the user no
    /// longer has, and point `config` at Gate's pick for the sign-ins
    /// left. Returns `true` when the selected model changed.
    pub fn ensure_usable(&mut self, config: &mut AgentLoopConfig) -> bool {
        if !self.usable(&self.current) {
            self.apply(config, GATE_MODEL_ID);
            return true;
        }
        self.refresh_route(config);
        false
    }

    /// Hint shown when the next turn's local model can't run, or `None`
    /// when it's ready.
    pub fn local_hint(&self) -> Option<String> {
        let model = self.turn_model();
        let name = local_model_name(&model)?;
        let setup = format!(
            "Start it with `OLLAMA_CONTEXT_LENGTH={} ollama serve`, then `ollama pull {name}`.",
            model_catalog::local_context_tokens()
        );
        match &self.local_status {
            LocalServerStatus::Ready { .. } if self.local_installed(&model) => None,
            LocalServerStatus::Ready { .. } => Some(format!(
                "Local model {name} is not pulled yet. Run `ollama pull {name}`, \
                 or use /login to use cloud models."
            )),
            LocalServerStatus::Outdated { version } => Some(format!(
                "Ollama {version} at {} is too old (Aura needs 0.14+ for the Messages API). \
                 Update Ollama, or use /login to use cloud models.",
                self.local_url
            )),
            LocalServerStatus::Unreachable => Some(format!(
                "No local model server at {}. Install Ollama (https://ollama.com). {setup} \
                 Or use /login to use cloud models.",
                self.local_url
            )),
        }
    }

    /// Display label for `id`.
    pub fn label(id: &str) -> String {
        if id == GATE_MODEL_ID {
            return GATE_LABEL.to_string();
        }
        let backend = backend_for(id).label();
        format!("{} ({backend})", local_model_name(id).unwrap_or(id))
    }

    /// Display label for the selected model, with Gate's pick:
    /// `Gate → claude-sonnet-4-6 (claude plan)`.
    pub fn display(&self) -> String {
        if self.is_gate() {
            format!(
                "{GATE_LABEL} → {}",
                model_catalog::route_label(&self.turn_model())
            )
        } else {
            Self::label(&self.current)
        }
    }

    fn apply(&mut self, config: &mut AgentLoopConfig, id: &str) {
        id.clone_into(&mut self.current);
        let model = self.turn_model();
        self.limits().configure(config, &model);
    }

    /// Limits for configuring other agent loops to run a given model.
    pub const fn limits(&self) -> ModelLimits {
        ModelLimits(self.router_limits)
    }

    fn persist(&self) {
        let Some(path) = &self.prefs_path else {
            return;
        };
        let prefs = TuiPrefs {
            model: Some(self.current.clone()),
            thinking: self
                .thinking
                .map(|effort| thinking_name(effort).to_string()),
        };
        let result = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                let json = serde_json::to_vec_pretty(&prefs).map_err(std::io::Error::other)?;
                std::fs::write(path, json)
            });
        if let Err(e) = result {
            warn!(error = %e, path = %path.display(), "Failed to persist model selection");
        }
    }
}

/// What it takes to point an agent loop at a model without changing the
/// terminal's selected model (used for permanent agents with their own
/// `model`).
#[derive(Debug, Clone, Copy)]
pub struct ModelLimits(RouterLimits);

/// The effort Gate chose for the work `config` runs, while the effort is
/// Gate's to choose: `None` when the user pinned one or Gate chose none.
pub fn routed_effort(config: &AgentLoopConfig) -> Option<ThinkingEffort> {
    config
        .user_thinking_effort
        .is_none()
        .then_some(config.routed_thinking_effort)
        .flatten()
}

impl ModelLimits {
    /// Configure `config` to run model `id`.
    pub fn configure(self, config: &mut AgentLoopConfig, id: &str) {
        let limits = self.0;
        config.model = id.to_string();
        config.routed_thinking_effort = None;
        config.max_context_tokens = Some(aura_engine::model_context::context_window_for_model(id));
        match backend_for(id) {
            ModelBackend::Local => {
                config.stream_timeout = LOCAL_STREAM_TIMEOUT;
                config.stream_event_timeout = LOCAL_STREAM_TIMEOUT;
            }
            ModelBackend::Router
            | ModelBackend::Claude
            | ModelBackend::Chatgpt
            | ModelBackend::OpenRouter => {
                config.stream_timeout = limits.stream_timeout;
                config.stream_event_timeout = limits.stream_event_timeout;
            }
        }
    }

    /// Point `config` at the model and reasoning effort Gate's `decision`
    /// chose.
    pub fn route(self, config: &mut AgentLoopConfig, decision: &aura_agent_gate::Decision) {
        if config.model != decision.model.id {
            self.configure(config, &decision.model.id);
        }
        config.routed_thinking_effort = Some(decision.effort);
    }

    /// Limits taken from `config` as the router settings.
    #[cfg(test)]
    pub(crate) const fn of(config: &AgentLoopConfig) -> Self {
        Self(RouterLimits {
            stream_timeout: config.stream_timeout,
            stream_event_timeout: config.stream_event_timeout,
        })
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

fn prefs_path() -> Option<PathBuf> {
    aura_config::AuraHome::resolve()
        .ok()
        .map(|home| home.path.join("tui.json"))
}

fn load_prefs(path: &std::path::Path) -> Option<TuiPrefs> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Beside `tui.json`: the Claude plan's model list as it last loaded,
/// used when it can't be loaded. The built-in list is older and would
/// leave Gate routing among models the plan has since replaced.
const PLAN_MODELS_FILE: &str = "plan_models.json";

/// A plan model as saved in [`PLAN_MODELS_FILE`].
#[derive(Debug, Serialize, Deserialize)]
struct SavedPlanModel {
    id: String,
    label: String,
}

fn save_plan_models(path: &std::path::Path, models: &[ModelOption]) {
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
fn load_plan_models(path: &std::path::Path) -> Vec<ModelOption> {
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
const OPENROUTER_MODELS_FILE: &str = "openrouter_models.json";

/// An OpenRouter model as saved in [`OPENROUTER_MODELS_FILE`].
#[derive(Debug, Serialize, Deserialize)]
struct SavedOpenRouterModel {
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
struct SavedPrice {
    input: f64,
    output: f64,
    cache_write: f64,
    cache_read: f64,
}

fn save_openrouter_models(path: &std::path::Path, entries: &[(ModelOption, LiveModelInfo)]) {
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
fn load_openrouter_models(path: &std::path::Path) -> Vec<(ModelOption, LiveModelInfo)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL: &str = "local/qwen3:4b";

    #[test]
    fn startup_defaults_to_gate() {
        assert_eq!(resolve_startup_model(None, None, |_| false), GATE_MODEL_ID);
        assert_eq!(
            resolve_startup_model(Some("  "), None, |_| true),
            GATE_MODEL_ID
        );
    }

    #[test]
    fn startup_env_model_wins() {
        assert_eq!(
            resolve_startup_model(Some("aura-claude-opus-4-7"), Some(LOCAL), |_| false),
            "aura-claude-opus-4-7"
        );
    }

    #[test]
    fn startup_persisted_model_requires_its_sign_in() {
        let guest = AuthSummary::default();
        let zos = signed_in(Some("alice"), &[]);
        let claude = signed_in(None, &[ProviderKind::Claude]);
        let start = |persisted: &str, auth: &AuthSummary| {
            resolve_startup_model(None, Some(persisted), |id| usable(auth, id))
        };
        assert_eq!(start("aura-claude-sonnet-4-6", &guest), GATE_MODEL_ID);
        assert_eq!(
            start("aura-claude-sonnet-4-6", &zos),
            "aura-claude-sonnet-4-6"
        );
        assert_eq!(start("local/llama3.2:3b", &guest), "local/llama3.2:3b");
        assert_eq!(start("claude/claude-sonnet-4-6", &zos), GATE_MODEL_ID);
        assert_eq!(
            start("claude/claude-sonnet-4-6", &claude),
            "claude/claude-sonnet-4-6"
        );
        assert_eq!(start(GATE_MODEL_ID, &guest), GATE_MODEL_ID);
    }

    fn signed_in(user: Option<&str>, plans: &[ProviderKind]) -> AuthSummary {
        AuthSummary {
            zos_user: user.map(str::to_string),
            subscriptions: plans.to_vec(),
            ..AuthSummary::default()
        }
    }

    fn selection_on(auth: AuthSummary, model: &str) -> (ModelSelection, AgentLoopConfig) {
        let mut config = AgentLoopConfig::for_agent("seed");
        let mut selection = ModelSelection {
            current: String::new(),
            default_local: LOCAL.to_string(),
            local_url: "http://127.0.0.1:9".to_string(),
            local_status: LocalServerStatus::Ready {
                version: "0.14.0".to_string(),
                models: vec!["qwen3:4b".to_string()],
            },
            auth,
            plan_models: Vec::new(),
            dropped: BTreeSet::new(),
            router_limits: RouterLimits {
                stream_timeout: config.stream_timeout,
                stream_event_timeout: config.stream_event_timeout,
            },
            thinking: None,
            prefs_path: None,
        };
        selection.apply(&mut config, model);
        (selection, config)
    }

    fn selection(user: Option<&str>) -> (ModelSelection, AgentLoopConfig) {
        selection_on(signed_in(user, &[]), LOCAL)
    }

    #[test]
    fn a_plan_list_that_fails_to_load_falls_back_to_the_one_saved_last() {
        let home = tempfile::tempdir().unwrap();
        let claude = signed_in(None, &[ProviderKind::Claude]);
        let (mut selection, _) = selection_on(claude, GATE_MODEL_ID);
        let ids = |selection: &ModelSelection| -> Vec<String> {
            selection
                .gate_candidates()
                .into_iter()
                .filter(|c| c.backend == ModelBackend::Claude)
                .map(|c| c.id)
                .collect()
        };

        let note = selection
            .plans_listed(Err("timed out".to_string()))
            .unwrap();
        assert!(note.contains("timed out") && note.contains("built-in list"));
        assert!(ids(&selection).contains(&"claude/claude-sonnet-4-6".to_string()));

        selection.prefs_path = Some(home.path().join("tui.json"));
        let live = ["claude/claude-haiku-5-5", "claude/claude-sonnet-5-5"].map(|id| ModelOption {
            id: id.to_string(),
            label: id.to_string(),
            backend: ModelBackend::Claude,
            installed: true,
            tier: infer_tier(id),
        });
        assert_eq!(selection.plans_listed(Ok(live.to_vec())), None);
        assert_eq!(ids(&selection), live.clone().map(|m| m.id));

        let (mut later, _) = selection_on(signed_in(None, &[ProviderKind::Claude]), GATE_MODEL_ID);
        later.prefs_path = Some(home.path().join("tui.json"));
        let note = later.plans_listed(Err("timed out".to_string())).unwrap();
        assert!(note.contains("saved when it last loaded"), "{note}");
        assert_eq!(ids(&later), live.map(|m| m.id), "not the built-in list");
        assert_eq!(later.classifier_model(), "claude/claude-haiku-5-5");
    }

    #[test]
    fn an_openrouter_list_that_fails_to_load_falls_back_to_the_one_saved_last() {
        let home = tempfile::tempdir().unwrap();
        let openrouter = || signed_in(None, &[ProviderKind::OpenRouter]);
        // The selector's entries, not Gate's candidates: Gate leaves out
        // OpenRouter models without a curated price, as these are.
        let ids = |selection: &ModelSelection| -> Vec<String> {
            selection
                .options()
                .into_iter()
                .filter(|o| backend_for(&o.id) == ModelBackend::OpenRouter)
                .map(|o| o.id)
                .collect()
        };
        let live: Vec<(ModelOption, LiveModelInfo)> = [
            "openrouter/acme/saved-test-fast",
            "openrouter/acme/saved-test-pro",
        ]
        .into_iter()
        .map(|id| {
            let option = ModelOption {
                id: id.to_string(),
                label: id.to_string(),
                backend: ModelBackend::OpenRouter,
                installed: true,
                tier: infer_tier(id),
            };
            let info = LiveModelInfo {
                context_window: Some(123_456),
                max_output: Some(8_192),
                price: Some(LivePrice {
                    input: 1.0,
                    output: 2.0,
                    cache_write: 1.0,
                    cache_read: 0.5,
                }),
            };
            (option, info)
        })
        .collect();
        let live_ids: Vec<String> = live.iter().map(|(o, _)| o.id.clone()).collect();

        let (mut selection, _) = selection_on(openrouter(), GATE_MODEL_ID);
        selection.prefs_path = Some(home.path().join("tui.json"));
        assert_eq!(selection.openrouter_listed(Ok(live)), None);
        assert_eq!(ids(&selection), live_ids);
        assert!(home.path().join(OPENROUTER_MODELS_FILE).exists());

        let (mut later, _) = selection_on(openrouter(), GATE_MODEL_ID);
        later.prefs_path = Some(home.path().join("tui.json"));
        let note = later
            .openrouter_listed(Err("timed out".to_string()))
            .unwrap();
        assert!(note.contains("timed out"), "{note}");
        assert!(note.contains("OpenRouter"), "{note}");
        assert!(note.contains("saved when it last loaded"), "{note}");
        assert!(
            note.contains("/model openrouter/<vendor>/<model>"),
            "{note}"
        );
        assert_eq!(ids(&later), live_ids, "not the built-in list");
        let info = model_catalog::live_model_info("openrouter/acme/saved-test-pro")
            .expect("saved limits published");
        assert_eq!(info.context_window, Some(123_456));
        assert_eq!(info.max_output, Some(8_192));
        assert_eq!(info.price.map(|p| p.output), Some(2.0));

        let (mut fresh, _) = selection_on(openrouter(), GATE_MODEL_ID);
        let note = fresh.openrouter_listed(Err("offline".to_string())).unwrap();
        assert!(note.contains("built-in list"), "{note}");
    }

    #[test]
    fn openrouter_options_are_grouped_by_vendor() {
        assert_eq!(
            group_heading(
                ModelBackend::OpenRouter,
                "openrouter/anthropic/claude-sonnet-4.6"
            ),
            "OpenRouter · anthropic"
        );
        let (selection, _) = selection_on(signed_in(None, &[ProviderKind::OpenRouter]), LOCAL);
        let options = selection.options();
        let entry = options
            .iter()
            .find(|o| o.id.starts_with("openrouter/anthropic/"))
            .expect("curated OpenRouter anthropic model listed");
        assert_eq!(entry.group, "OpenRouter · anthropic");
    }

    #[test]
    fn openrouter_models_need_the_openrouter_sign_in() {
        let id = "openrouter/anthropic/claude-sonnet-4.6";
        assert!(!usable(
            &signed_in(Some("alice"), &[ProviderKind::Claude]),
            id
        ));
        assert!(usable(&signed_in(None, &[ProviderKind::OpenRouter]), id));

        let (mut selection, mut config) = selection(Some("alice"));
        let err = selection.select(&mut config, id).unwrap_err();
        assert!(err.contains("/login openrouter"), "{err}");
        assert!(err.contains("OpenRouter account"), "{err}");
    }

    #[test]
    fn a_thinking_level_pins_every_turn_and_survives_a_model_switch() {
        let (mut selection, mut config) = selection(Some("alice"));
        assert_eq!(config.user_thinking_effort, None);
        assert_eq!(selection.thinking_display(), "auto");

        selection
            .set_thinking(&mut config, Some("high"))
            .expect("a level");
        assert_eq!(config.user_thinking_effort, Some(ThinkingEffort::High));
        selection
            .select(&mut config, "aura-claude-sonnet-4-6")
            .expect("signed-in cloud model");
        assert_eq!(config.user_thinking_effort, Some(ThinkingEffort::High));
        let aura_ui_terminal::UiCommand::SetModels { thinking, .. } = selection.ui_command() else {
            panic!("expected SetModels");
        };
        assert_eq!(thinking.as_deref(), Some("high"));

        selection.set_thinking(&mut config, Some("off")).unwrap();
        assert_eq!(selection.thinking_display(), "off");
        assert!(selection.set_thinking(&mut config, Some("lots")).is_err());
        assert_eq!(config.user_thinking_effort, Some(ThinkingEffort::Off));

        selection.set_thinking(&mut config, None).unwrap();
        assert_eq!(config.user_thinking_effort, None);
    }

    #[test]
    fn the_thinking_level_is_saved_with_the_model() {
        let home = tempfile::tempdir().unwrap();
        let (mut selection, mut config) = selection(None);
        selection.prefs_path = Some(home.path().join("tui.json"));
        selection.set_thinking(&mut config, Some("max")).unwrap();
        let prefs = load_prefs(&home.path().join("tui.json")).expect("saved");
        assert_eq!(prefs.thinking.as_deref(), Some("max"));
        assert_eq!(prefs.model.as_deref(), Some(LOCAL));
    }

    #[test]
    fn guest_cannot_select_cloud_model() {
        let (mut selection, mut config) = selection(None);
        assert!(selection
            .select(&mut config, "aura-claude-sonnet-4-6")
            .is_err());
        assert_eq!(config.model, LOCAL);
    }

    #[test]
    fn switching_backends_swaps_limits() {
        let (mut selection, mut config) = selection(Some("alice"));
        assert_eq!(
            config.max_context_tokens,
            Some(model_catalog::local_context_tokens())
        );

        selection
            .select(&mut config, "aura-claude-sonnet-4-6")
            .expect("logged-in user may pick cloud models");
        assert_eq!(config.model, "aura-claude-sonnet-4-6");
        assert_eq!(config.max_context_tokens, Some(1_000_000));
    }

    #[test]
    fn gate_runs_turns_on_its_pick_with_that_models_limits() {
        let (selection, config) = selection_on(signed_in(Some("alice"), &[]), GATE_MODEL_ID);
        assert!(selection.is_gate());
        assert_eq!(selection.current(), GATE_MODEL_ID);
        assert_eq!(config.model, "aura-claude-sonnet-4-6");
        assert_eq!(config.max_context_tokens, Some(1_000_000));
        assert_eq!(selection.display(), "Gate → aura-claude-sonnet-4-6 (cloud)");
    }

    #[test]
    fn gate_follows_sign_ins() {
        let (mut selection, mut config) = selection_on(AuthSummary::default(), GATE_MODEL_ID);
        assert_eq!(config.model, LOCAL, "nothing signed in: local models only");

        selection.set_auth(signed_in(Some("alice"), &[]));
        assert!(selection.refresh_route(&mut config));
        assert_eq!(config.model, "aura-claude-sonnet-4-6");

        selection.set_auth(signed_in(Some("alice"), &[ProviderKind::Claude]));
        assert!(selection.refresh_route(&mut config));
        assert_eq!(
            config.model, "claude/claude-sonnet-4-6",
            "the plan route wins over the cloud router"
        );
        assert!(!selection.refresh_route(&mut config));

        selection.set_auth(AuthSummary::default());
        assert!(!selection.ensure_usable(&mut config));
        assert_eq!(config.model, LOCAL);
    }

    #[test]
    fn gate_with_nothing_runnable_points_at_the_local_default() {
        let (mut selection, mut config) = selection_on(AuthSummary::default(), GATE_MODEL_ID);
        selection.local_status = LocalServerStatus::Unreachable;
        selection.refresh_route(&mut config);
        assert_eq!(selection.route(), None);
        assert_eq!(config.model, LOCAL);
        assert!(selection
            .local_hint()
            .is_some_and(|hint| hint.contains("No local model server")));
    }

    #[test]
    fn a_refused_model_is_dropped_until_sign_ins_change() {
        let claude = signed_in(None, &[ProviderKind::Claude]);
        let (mut selection, mut config) = selection_on(claude.clone(), GATE_MODEL_ID);
        assert_eq!(config.model, "claude/claude-sonnet-4-6");

        selection.drop_model("claude/claude-sonnet-4-6");
        assert!(!selection
            .gate_candidates()
            .iter()
            .any(|c| c.id == "claude/claude-sonnet-4-6"));
        assert!(selection.refresh_route(&mut config));
        assert_eq!(config.model, "claude/claude-opus-4-7");

        selection.set_auth(claude);
        selection.refresh_route(&mut config);
        assert_eq!(config.model, "claude/claude-sonnet-4-6");
    }

    #[test]
    fn losing_a_sign_in_switches_to_gate() {
        let (mut selection, mut config) = selection(Some("alice"));
        selection
            .select(&mut config, "aura-claude-sonnet-4-6")
            .expect("select cloud");
        selection.set_auth(AuthSummary::default());
        assert!(selection.ensure_usable(&mut config));
        assert_eq!(selection.current(), GATE_MODEL_ID);
        assert_eq!(config.model, LOCAL);
    }

    #[test]
    fn plan_models_need_their_own_link() {
        let (mut selection, mut config) = selection(Some("alice"));
        let err = selection
            .select(&mut config, "claude/claude-sonnet-4-6")
            .unwrap_err();
        assert!(err.contains("/login claude"), "{err}");

        selection.set_auth(signed_in(Some("alice"), &[ProviderKind::Claude]));
        selection
            .select(&mut config, "claude/claude-sonnet-4-6")
            .expect("linked plan");
        assert_eq!(config.max_context_tokens, Some(1_000_000));

        let options = selection.options();
        let plan = options
            .iter()
            .find(|o| o.id == "chatgpt/gpt-5.4")
            .expect("chatgpt models listed");
        assert!(!plan.available);
        assert_eq!(plan.provider.as_deref(), Some("chatgpt"));

        selection.set_auth(signed_in(Some("alice"), &[]));
        assert!(selection.ensure_usable(&mut config));
        assert_eq!(selection.current(), GATE_MODEL_ID);
        assert_eq!(
            config.model, "aura-claude-sonnet-4-6",
            "Gate routes across the sign-ins left"
        );
    }

    #[test]
    fn the_cheapest_fast_model_classifies_else_the_local_default() {
        let (claude, _) = selection_on(signed_in(None, &[ProviderKind::Claude]), LOCAL);
        assert_eq!(claude.classifier_model(), "claude/claude-haiku-4-5");
        let (guest, _) = selection_on(AuthSummary::default(), GATE_MODEL_ID);
        assert_eq!(guest.classifier_model(), LOCAL);
    }

    #[test]
    fn gate_is_listed_first() {
        let (selection, _) = selection(None);
        let options = selection.options();
        assert_eq!(options[0].id, GATE_MODEL_ID);
        assert_eq!(options[0].label, "Gate");
        assert!(options[0].available);
    }

    #[test]
    fn the_status_line_gets_gates_pick() {
        let claude = signed_in(None, &[ProviderKind::Claude]);
        let (gate, _) = selection_on(claude, GATE_MODEL_ID);
        let aura_ui_terminal::UiCommand::SetModels { current, route, .. } = gate.ui_command()
        else {
            panic!("expected SetModels");
        };
        assert_eq!(current, GATE_MODEL_ID);
        assert_eq!(route.as_deref(), Some("claude-sonnet-4-6 (claude plan)"));

        let (pinned, _) = selection(None);
        let aura_ui_terminal::UiCommand::SetModels { route, .. } = pinned.ui_command() else {
            panic!("expected SetModels");
        };
        assert_eq!(route, None);
    }

    #[test]
    fn ready_installed_model_has_no_hint() {
        let (mut selection, _) = selection(None);
        assert_eq!(selection.local_hint(), None);
        selection.local_status = LocalServerStatus::Unreachable;
        assert!(selection
            .local_hint()
            .is_some_and(|hint| hint.contains("ollama pull qwen3:4b")));
    }

    #[test]
    fn cloud_model_has_no_local_hint_when_ollama_is_down() {
        let (mut selection, mut config) = selection(Some("alice"));
        selection
            .select(&mut config, "aura-claude-sonnet-4-6")
            .expect("signed-in cloud model");
        selection.local_status = LocalServerStatus::Unreachable;
        assert_eq!(selection.local_hint(), None);
    }

    #[test]
    fn options_include_unlisted_current_model() {
        let (mut selection, mut config) = selection(None);
        selection
            .select(&mut config, "local/phi4-mini")
            .expect("local models need no login");
        let options = selection.options();
        let entry = options
            .iter()
            .find(|o| o.id == "local/phi4-mini")
            .expect("current model listed");
        assert!(entry.local);
        assert!(!entry.installed);
    }
}
