//! Terminal model selection: startup default, `/model` switching, and
//! persistence of the last choice.
//!
//! The TUI defaults to a small local model served by Ollama so it works
//! without an account; `/login` unlocks the aura-router cloud models.

use std::path::PathBuf;
use std::time::Duration;

use aura_agent::AgentLoopConfig;
use aura_model_reasoner::model_catalog::{
    self, backend_for, local_model_name, LocalServerStatus, ModelBackend, ModelOption,
};
use aura_terminal::events::ModelOptionView;
use serde::{Deserialize, Serialize};
use tracing::warn;

const LOCAL_STREAM_TIMEOUT: Duration = Duration::from_secs(600);

/// Persisted terminal preferences (`AURA_HOME/tui.json`).
#[derive(Debug, Default, Serialize, Deserialize)]
struct TuiPrefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
}

/// Router-side agent-loop limits, captured before any local model is applied.
#[derive(Debug, Clone, Copy)]
struct RouterLimits {
    max_tokens: u32,
    stream_timeout: Duration,
    stream_event_timeout: Duration,
}

/// Model selection state owned by the terminal event loop.
pub struct ModelSelection {
    current: String,
    default_local: String,
    local_url: String,
    local_status: LocalServerStatus,
    user: Option<String>,
    router_limits: RouterLimits,
    prefs_path: Option<PathBuf>,
}

impl ModelSelection {
    /// Resolve the startup model and apply it to `config`.
    ///
    /// `user` is the logged-in display name (`None` for guests).
    pub fn init(config: &mut AgentLoopConfig, user: Option<String>) -> Self {
        let prefs_path = prefs_path();
        let persisted = prefs_path
            .as_deref()
            .and_then(load_prefs)
            .and_then(|p| p.model);
        let env_model = std::env::var("AURA_DEFAULT_MODEL")
            .or_else(|_| std::env::var("AURA_ANTHROPIC_MODEL"))
            .ok();
        let default_local = model_catalog::default_local_model_id();
        let current = resolve_startup_model(
            env_model.as_deref(),
            persisted.as_deref(),
            &default_local,
            user.is_some(),
        );
        let mut selection = Self {
            current: String::new(),
            default_local,
            local_url: model_catalog::local_base_url(),
            local_status: LocalServerStatus::Unreachable,
            user,
            router_limits: RouterLimits {
                max_tokens: config.max_tokens,
                stream_timeout: config.stream_timeout,
                stream_event_timeout: config.stream_event_timeout,
            },
            prefs_path,
        };
        selection.apply(config, &current);
        selection
    }

    /// Model id used for the next turn.
    pub fn current(&self) -> &str {
        &self.current
    }

    /// Whether the user is logged in to aura-router.
    pub const fn logged_in(&self) -> bool {
        self.user.is_some()
    }

    /// Update the logged-in user (`None` after logout).
    pub fn set_user(&mut self, user: Option<String>) {
        self.user = user;
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
        let mut options = model_catalog::catalog(&self.default_local, self.installed_local());
        if !options.iter().any(|o| o.id == self.current) {
            let backend = backend_for(&self.current);
            options.push(ModelOption {
                id: self.current.clone(),
                label: local_model_name(&self.current)
                    .unwrap_or(&self.current)
                    .to_string(),
                backend,
                installed: backend == ModelBackend::Router || self.local_installed(&self.current),
            });
        }
        options
            .into_iter()
            .map(|o| ModelOptionView {
                local: o.backend == ModelBackend::Local,
                id: o.id,
                label: o.label,
                installed: o.installed,
            })
            .collect()
    }

    /// `UiCommand::SetModels` payload for the current state.
    pub fn ui_command(&self) -> aura_terminal::UiCommand {
        aura_terminal::UiCommand::SetModels {
            options: self.options(),
            current: self.current.clone(),
            user: self.user.clone(),
        }
    }

    fn local_installed(&self, id: &str) -> bool {
        local_model_name(id)
            .is_some_and(|name| model_catalog::is_installed(self.installed_local(), name))
    }

    /// Switch to `id`. Returns a user-facing problem when the switch is refused.
    pub fn select(&mut self, config: &mut AgentLoopConfig, id: &str) -> Result<(), String> {
        let id = id.trim();
        if id.is_empty() {
            return Err("Model id is empty".to_string());
        }
        if backend_for(id) == ModelBackend::Router && !self.logged_in() {
            return Err(format!(
                "{id} is a cloud model — use /login first, or pick a local model"
            ));
        }
        self.apply(config, id);
        self.persist();
        Ok(())
    }

    /// Fall back to the default local model when the current model needs a login.
    /// Returns `true` when the model changed.
    pub fn ensure_usable(&mut self, config: &mut AgentLoopConfig) -> bool {
        if backend_for(&self.current) == ModelBackend::Router && !self.logged_in() {
            let local = self.default_local.clone();
            self.apply(config, &local);
            return true;
        }
        false
    }

    /// Hint shown when the current local model can't run, or `None` when it's ready.
    pub fn local_hint(&self) -> Option<String> {
        let name = local_model_name(&self.current)?;
        let setup = format!(
            "Start it with `OLLAMA_CONTEXT_LENGTH={} ollama serve`, then `ollama pull {name}`.",
            model_catalog::local_context_tokens()
        );
        match &self.local_status {
            LocalServerStatus::Ready { .. } if self.local_installed(&self.current) => None,
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
        let backend = backend_for(id).label();
        format!("{} ({backend})", local_model_name(id).unwrap_or(id))
    }

    fn apply(&mut self, config: &mut AgentLoopConfig, id: &str) {
        id.clone_into(&mut self.current);
        self.limits().configure(config, id);
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

impl ModelLimits {
    /// Configure `config` to run model `id`.
    pub fn configure(self, config: &mut AgentLoopConfig, id: &str) {
        let limits = self.0;
        config.model = id.to_string();
        config.max_context_tokens = Some(aura_engine::model_context::context_window_for_model(id));
        match backend_for(id) {
            ModelBackend::Local => {
                config.max_tokens = limits
                    .max_tokens
                    .min(model_catalog::DEFAULT_LOCAL_MAX_OUTPUT_TOKENS);
                config.stream_timeout = LOCAL_STREAM_TIMEOUT;
                config.stream_event_timeout = LOCAL_STREAM_TIMEOUT;
            }
            ModelBackend::Router => {
                config.max_tokens = limits.max_tokens;
                config.stream_timeout = limits.stream_timeout;
                config.stream_event_timeout = limits.stream_event_timeout;
            }
        }
    }

    /// Limits taken from `config` as the router settings.
    #[cfg(test)]
    pub(crate) const fn of(config: &AgentLoopConfig) -> Self {
        Self(RouterLimits {
            max_tokens: config.max_tokens,
            stream_timeout: config.stream_timeout,
            stream_event_timeout: config.stream_event_timeout,
        })
    }
}

/// Startup model: explicit `AURA_DEFAULT_MODEL` wins, then the persisted
/// choice (unless it needs a login the user doesn't have), then the
/// default local model.
fn resolve_startup_model(
    env_model: Option<&str>,
    persisted: Option<&str>,
    default_local: &str,
    logged_in: bool,
) -> String {
    if let Some(model) = env_model.map(str::trim).filter(|m| !m.is_empty()) {
        return model.to_string();
    }
    if let Some(model) = persisted.map(str::trim).filter(|m| !m.is_empty()) {
        if logged_in || backend_for(model) == ModelBackend::Local {
            return model.to_string();
        }
    }
    default_local.to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL: &str = "local/qwen3:4b";

    #[test]
    fn startup_defaults_to_local_model() {
        assert_eq!(resolve_startup_model(None, None, LOCAL, false), LOCAL);
        assert_eq!(resolve_startup_model(Some("  "), None, LOCAL, true), LOCAL);
    }

    #[test]
    fn startup_env_model_wins() {
        assert_eq!(
            resolve_startup_model(Some("aura-claude-opus-4-7"), Some(LOCAL), LOCAL, false),
            "aura-claude-opus-4-7"
        );
    }

    #[test]
    fn startup_persisted_cloud_model_requires_login() {
        let persisted = Some("aura-claude-sonnet-4-6");
        assert_eq!(resolve_startup_model(None, persisted, LOCAL, false), LOCAL);
        assert_eq!(
            resolve_startup_model(None, persisted, LOCAL, true),
            "aura-claude-sonnet-4-6"
        );
        assert_eq!(
            resolve_startup_model(None, Some("local/llama3.2:3b"), LOCAL, false),
            "local/llama3.2:3b"
        );
    }

    fn selection(user: Option<&str>) -> (ModelSelection, AgentLoopConfig) {
        let mut config = AgentLoopConfig::for_agent("seed");
        let mut selection = ModelSelection {
            current: String::new(),
            default_local: LOCAL.to_string(),
            local_url: "http://127.0.0.1:9".to_string(),
            local_status: LocalServerStatus::Ready {
                version: "0.14.0".to_string(),
                models: vec!["qwen3:4b".to_string()],
            },
            user: user.map(str::to_string),
            router_limits: RouterLimits {
                max_tokens: config.max_tokens,
                stream_timeout: config.stream_timeout,
                stream_event_timeout: config.stream_event_timeout,
            },
            prefs_path: None,
        };
        selection.apply(&mut config, LOCAL);
        (selection, config)
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
            config.max_tokens,
            model_catalog::DEFAULT_LOCAL_MAX_OUTPUT_TOKENS
        );
        assert_eq!(
            config.max_context_tokens,
            Some(model_catalog::local_context_tokens())
        );

        selection
            .select(&mut config, "aura-claude-sonnet-4-6")
            .expect("logged-in user may pick cloud models");
        assert_eq!(config.model, "aura-claude-sonnet-4-6");
        assert_eq!(config.max_tokens, 16_384);
        assert_eq!(config.max_context_tokens, Some(1_000_000));
    }

    #[test]
    fn logout_falls_back_to_local() {
        let (mut selection, mut config) = selection(Some("alice"));
        selection
            .select(&mut config, "aura-claude-sonnet-4-6")
            .expect("select cloud");
        selection.set_user(None);
        assert!(selection.ensure_usable(&mut config));
        assert_eq!(config.model, LOCAL);
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
