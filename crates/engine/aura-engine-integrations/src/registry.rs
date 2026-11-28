//! The [`Registry`]: every integration, and what is done the same way
//! for all of them.
//!
//! ## Invariants
//!
//! - Ids are unique: [`Registry::add`] refuses a second one.
//! - [`Registry::connect`] stores a credential only after the
//!   integration's health check passed with it; a failed check stores
//!   nothing and leaves any earlier credential as it was.
//! - Credential and settings files are only touched off the async
//!   threads (`spawn_blocking`).
//! - Every status, health check and tool call has a time limit.

use crate::context::{IntegrationContext, SecretSource};
use crate::error::IntegrationError;
use crate::id::IntegrationId;
use crate::integration::{HealthReport, Integration, Status};
use crate::manifest::{ConfigField, Manifest};
use crate::secret::{lock, Secret, SecretStore};
use crate::settings::{Access, Settings};
use crate::tools::IntegrationTool;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a status, health check or tool call may take by default.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// One integration as the Integrations panel shows it.
#[derive(Debug, Clone)]
pub struct IntegrationView {
    /// What it is
    pub manifest: Manifest,
    /// Whether it is ready
    pub status: Status,
    /// Each config field with its value, if set
    pub config: Vec<(ConfigField, Option<String>)>,
    /// Who may use it
    pub access: Access,
    /// Whether a credential is stored for it (so Disconnect has
    /// something to remove)
    pub stored: bool,
}

/// Every integration of this process.
pub struct Registry {
    integrations: Vec<Arc<dyn Integration>>,
    secrets: Arc<dyn SecretStore>,
    settings: Mutex<Settings>,
    settings_path: Option<PathBuf>,
    http: reqwest::Client,
    timeout: Duration,
}

impl Registry {
    /// An empty registry keeping credentials in `secrets` and settings
    /// in memory.
    #[must_use]
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            integrations: Vec::new(),
            secrets,
            settings: Mutex::new(Settings::default()),
            settings_path: None,
            http: reqwest::Client::new(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// This registry with its settings in `path`, loaded now.
    ///
    /// # Errors
    /// The file exists but can't be read or parsed.
    pub fn with_settings_file(mut self, path: PathBuf) -> Result<Self, IntegrationError> {
        self.settings = Mutex::new(Settings::load(&path)?);
        self.settings_path = Some(path);
        Ok(self)
    }

    /// This registry giving each call `timeout`.
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Add `integration`.
    ///
    /// # Errors
    /// [`IntegrationError::Duplicate`] when its id is taken.
    pub fn add(&mut self, integration: Arc<dyn Integration>) -> Result<(), IntegrationError> {
        let id = &integration.manifest().id;
        if self.get(id.as_str()).is_some() {
            return Err(IntegrationError::Duplicate(id.clone()));
        }
        self.integrations.push(integration);
        Ok(())
    }

    /// Every integration's manifest, in the order they were added.
    #[must_use]
    pub fn list(&self) -> Vec<&Manifest> {
        self.integrations.iter().map(|i| i.manifest()).collect()
    }

    /// The integration `id`.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Arc<dyn Integration>> {
        self.integrations
            .iter()
            .find(|integration| integration.manifest().id.as_str() == id)
    }

    fn find(&self, id: &str) -> Result<&Arc<dyn Integration>, IntegrationError> {
        self.get(id)
            .ok_or_else(|| IntegrationError::Unknown(id.to_string()))
    }

    /// The context `id` runs in now: its stored credential, else its
    /// first env variable set, and its config with defaults.
    ///
    /// # Errors
    /// `id` is unknown, or the credential store can't be read.
    pub async fn context(&self, id: &str) -> Result<IntegrationContext, IntegrationError> {
        let manifest = self.find(id)?.manifest().clone();
        let base = IntegrationContext::new(manifest.id.clone(), self.http.clone(), self.timeout)
            .with_config(self.config_of(&manifest));
        if let Some(stored) = self.stored(&manifest.id).await? {
            return Ok(base.with_secret(stored, SecretSource::Stored));
        }
        let from_env = manifest.auth.key().and_then(|key| {
            key.env.iter().find_map(|name| {
                std::env::var(name)
                    .ok()
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| (Secret::new(value), name.clone()))
            })
        });
        Ok(match from_env {
            Some((secret, name)) => base.with_secret(secret, SecretSource::Env(name)),
            None => base,
        })
    }

    fn config_of(&self, manifest: &Manifest) -> std::collections::BTreeMap<String, String> {
        let mut config = lock(&self.settings).entry(&manifest.id).config;
        for field in &manifest.config {
            if let Some(default) = &field.default {
                config
                    .entry(field.key.clone())
                    .or_insert_with(|| default.clone());
            }
        }
        config
    }

    async fn stored(&self, id: &IntegrationId) -> Result<Option<Secret>, IntegrationError> {
        let (secrets, id) = (Arc::clone(&self.secrets), id.clone());
        blocking(move || secrets.get(&id)).await
    }

    /// Whether `id` is ready, within the time limit.
    ///
    /// # Errors
    /// `id` is unknown or its credential can't be read.
    pub async fn status(&self, id: &str) -> Result<Status, IntegrationError> {
        let integration = self.find(id)?;
        let ctx = self.context(id).await?;
        Ok(
            match tokio::time::timeout(self.timeout, integration.status(&ctx)).await {
                Ok(status) => status,
                Err(_) => Status::Misconfigured {
                    why: format!("its status took longer than {}s", self.timeout.as_secs()),
                },
            },
        )
    }

    /// The panel's Test: `id`'s health check, within the time limit.
    ///
    /// # Errors
    /// The check failed or timed out, or `id` is unknown.
    pub async fn test(&self, id: &str) -> Result<HealthReport, IntegrationError> {
        let integration = self.find(id)?;
        let ctx = self.context(id).await?;
        ctx.timed(integration.health(&ctx)).await
    }

    /// Connect `id` with `secret`: check its health with it, and only
    /// then store it, sealed.
    ///
    /// # Errors
    /// `id` takes no key or token, `secret` is empty, the health check
    /// failed, or the store can't be written. Nothing is stored then.
    pub async fn connect(
        &self,
        id: &str,
        secret: Secret,
    ) -> Result<HealthReport, IntegrationError> {
        let integration = self.find(id)?;
        let manifest = integration.manifest();
        let Some(key) = manifest.auth.key() else {
            return Err(IntegrationError::BadConfig {
                id: manifest.id.clone(),
                key: "connect".to_string(),
                message: format!(
                    "{} takes no key: {}",
                    manifest.name,
                    how_to_sign_in(manifest)
                ),
            });
        };
        if secret.is_empty() {
            return Err(IntegrationError::BadConfig {
                id: manifest.id.clone(),
                key: "connect".to_string(),
                message: format!("the {} is empty", key.label.to_lowercase()),
            });
        }
        let ctx = self
            .context(id)
            .await?
            .with_secret(secret.clone(), SecretSource::Candidate);
        let report = ctx.timed(integration.health(&ctx)).await?;
        let (secrets, id) = (Arc::clone(&self.secrets), manifest.id.clone());
        blocking(move || secrets.put(&id, &secret)).await?;
        tracing::info!(integration = %manifest.id, "Integration connected");
        Ok(report)
    }

    /// Forget `id`'s stored credential; whether there was one. A
    /// credential from the environment or a CLI login stays.
    ///
    /// # Errors
    /// `id` is unknown or the store can't be written.
    pub async fn disconnect(&self, id: &str) -> Result<bool, IntegrationError> {
        let id = self.find(id)?.manifest().id.clone();
        let secrets = Arc::clone(&self.secrets);
        let removed = blocking({
            let id = id.clone();
            move || secrets.remove(&id)
        })
        .await?;
        tracing::info!(integration = %id, removed, "Integration disconnected");
        Ok(removed)
    }

    /// Set `id`'s config `key` to `value`; an empty value clears it.
    ///
    /// # Errors
    /// `id` or `key` is unknown, `value` doesn't fit the field, or the
    /// settings can't be saved.
    pub async fn configure(
        &self,
        id: &str,
        key: &str,
        value: &str,
    ) -> Result<(), IntegrationError> {
        let manifest = self.find(id)?.manifest();
        let bad = |message: String| IntegrationError::BadConfig {
            id: manifest.id.clone(),
            key: key.to_string(),
            message,
        };
        let field = manifest
            .field(key)
            .ok_or_else(|| bad("no such setting".to_string()))?;
        let value = value.trim();
        if let Some(why) = (!value.is_empty())
            .then(|| field.kind.reject(value))
            .flatten()
        {
            return Err(bad(why));
        }
        let id = manifest.id.clone();
        let (key, value) = (key.to_string(), value.to_string());
        self.change_settings(move |settings| {
            let config = &mut settings.entry_mut(&id).config;
            if value.is_empty() {
                config.remove(&key);
            } else {
                config.insert(key, value);
            }
        })
        .await
    }

    /// Who may use `id`.
    #[must_use]
    pub fn access(&self, id: &IntegrationId) -> Access {
        lock(&self.settings).entry(id).access
    }

    /// Let `access` use `id`.
    ///
    /// # Errors
    /// `id` is unknown or the settings can't be saved.
    pub async fn set_access(&self, id: &str, access: Access) -> Result<(), IntegrationError> {
        let id = self.find(id)?.manifest().id.clone();
        self.change_settings(move |settings| settings.entry_mut(&id).access = access)
            .await
    }

    async fn change_settings(
        &self,
        change: impl FnOnce(&mut Settings) + Send + 'static,
    ) -> Result<(), IntegrationError> {
        let changed = {
            let mut settings = lock(&self.settings);
            change(&mut settings);
            settings.clone()
        };
        let Some(path) = self.settings_path.clone() else {
            return Ok(());
        };
        blocking(move || changed.save(&path)).await
    }

    /// Every integration as the panel shows it, in order.
    pub async fn views(&self) -> Vec<IntegrationView> {
        let mut views = Vec::with_capacity(self.integrations.len());
        for integration in &self.integrations {
            views.push(self.view(integration.manifest()).await);
        }
        views
    }

    async fn view(&self, manifest: &Manifest) -> IntegrationView {
        let id = manifest.id.as_str();
        let status = self
            .status(id)
            .await
            .unwrap_or_else(|error| Status::Misconfigured {
                why: error.to_string(),
            });
        let config = self.config_of(manifest);
        IntegrationView {
            config: manifest
                .config
                .iter()
                .map(|field| (field.clone(), config.get(&field.key).cloned()))
                .collect(),
            access: self.access(&manifest.id),
            stored: self.stored(&manifest.id).await.ok().flatten().is_some(),
            status,
            manifest: manifest.clone(),
        }
    }

    /// The tools agent `agent` (hex id), a member of `channel` if any,
    /// gets: those of every connected integration its access allows.
    pub async fn tools_for(
        self: &Arc<Self>,
        agent: &str,
        channel: Option<&str>,
    ) -> Vec<IntegrationTool> {
        let mut tools = Vec::new();
        for integration in &self.integrations {
            let manifest = integration.manifest();
            if manifest.tools.is_empty() || !self.access(&manifest.id).allows(agent, channel) {
                continue;
            }
            if !self
                .status(manifest.id.as_str())
                .await
                .is_ok_and(|s| s.is_connected())
            {
                continue;
            }
            for spec in &manifest.tools {
                tools.push(IntegrationTool::new(
                    Arc::clone(self),
                    manifest,
                    spec,
                    (agent, channel),
                ));
            }
        }
        tools
    }

    /// [`Self::tools_for`] without calls, for code that can't wait:
    /// the tools of the integrations `connected` says are connected (a
    /// snapshot of [`Self::status`]) that `agent` may use. Each call
    /// still checks connection and access again.
    pub fn tools_where(
        self: &Arc<Self>,
        (agent, channel): (&str, Option<&str>),
        connected: impl Fn(&IntegrationId) -> bool,
    ) -> Vec<IntegrationTool> {
        self.integrations
            .iter()
            .map(|integration| integration.manifest())
            .filter(|manifest| {
                connected(&manifest.id) && self.access(&manifest.id).allows(agent, channel)
            })
            .flat_map(|manifest| {
                manifest.tools.iter().map(move |spec| {
                    IntegrationTool::new(Arc::clone(self), manifest, spec, (agent, channel))
                })
            })
            .collect()
    }

    /// Run `id`'s tool `tool` for `agent` in `channel`: checked against
    /// its access and connection again, within the tool's time limit.
    ///
    /// # Errors
    /// Unknown integration or tool, not allowed, not connected, timed
    /// out, or the tool failed.
    pub async fn call(
        &self,
        id: &str,
        tool: &str,
        args: serde_json::Value,
        (agent, channel): (&str, Option<&str>),
    ) -> Result<serde_json::Value, IntegrationError> {
        let integration = self.find(id)?;
        let manifest = integration.manifest();
        let spec = manifest
            .tool(tool)
            .ok_or_else(|| IntegrationError::UnknownTool {
                id: manifest.id.clone(),
                tool: tool.to_string(),
            })?;
        if !self.access(&manifest.id).allows(agent, channel) {
            return Err(IntegrationError::Denied {
                id: manifest.id.clone(),
            });
        }
        if let Status::NotConnected { hint } | Status::Misconfigured { why: hint } =
            self.status(id).await?
        {
            return Err(IntegrationError::NotConnected {
                id: manifest.id.clone(),
                hint,
            });
        }
        let ctx = self.context(id).await?;
        let ctx = ctx
            .clone()
            .with_timeout(spec.timeout.unwrap_or(ctx.timeout()));
        ctx.timed(integration.call_tool(&ctx, tool, args)).await
    }
}

/// How `manifest`'s integration signs in, when it takes no key.
fn how_to_sign_in(manifest: &Manifest) -> String {
    match &manifest.auth {
        crate::AuthKind::CliLogin { login, .. } => format!("run {login}"),
        crate::AuthKind::OAuth { .. } => "OAuth sign-in is not available yet".to_string(),
        crate::AuthKind::None | crate::AuthKind::ApiKey(_) => "it needs no sign-in".to_string(),
    }
}

/// Run blocking store IO off the async threads.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, IntegrationError> + Send + 'static,
) -> Result<T, IntegrationError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| IntegrationError::Store(format!("store task: {e}")))?
}
