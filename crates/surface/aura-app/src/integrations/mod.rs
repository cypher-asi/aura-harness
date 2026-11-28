//! This process's integrations: the [`Registry`] of every integration
//! (GitHub, boxd, and any added since), opened once at startup.
//!
//! - Credentials entered in the Integrations panel are sealed in
//!   `<data_dir>/integrations/secrets.sealed` with the local store's own
//!   key ([`crate::local_store::KeySource`], the OS keychain or
//!   `AURA_STATE_KEY_FILE`), opened after the local store made it. When that key can't be had, they are kept
//!   in memory for the session instead, with a WARN; never in plain
//!   text.
//! - Settings go in `<data_dir>/integrations/config.json`.
//! - Kernels are built synchronously, so which integrations are
//!   connected is kept as a snapshot ([`Integrations::refresh`], at
//!   startup and after every change), and their tools are registered on
//!   each agent's executor from it. Every call checks connection and
//!   access again.
//! - The GitHub token stored here is also kept for the REPO tab, which
//!   reads it after `GH_TOKEN` and `GITHUB_TOKEN`.

use aura_engine_integrations::builtin::{builtin, BuiltinOptions};
use aura_engine_integrations::{
    IntegrationId, MemorySecrets, Registry, SealedSecrets, Secret, SecretStore, SECRETS_FILE,
    SETTINGS_FILE,
};
use aura_exec_tools::ToolExecutor;
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};
use tracing::{info, warn};

/// The folder in the data directory integrations keep their files in.
pub const DIR: &str = "integrations";

/// The process's integrations, once opened.
static SERVICE: OnceLock<Arc<Integrations>> = OnceLock::new();

/// Every integration of this process and what is known about them now.
pub struct Integrations {
    registry: Arc<Registry>,
    /// Ids connected at the last [`Self::refresh`]
    connected: RwLock<HashSet<IntegrationId>>,
    /// GitHub's stored token, for the REPO tab
    github_token: RwLock<Option<Secret>>,
}

/// Open this process's integrations under `data_dir`, once; later calls
/// return the first. Starts a [`Integrations::refresh`] when a runtime
/// is running.
pub fn open(data_dir: &Path) -> Arc<Integrations> {
    let service = SERVICE.get_or_init(|| Arc::new(Integrations::new(data_dir)));
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        let service = Arc::clone(service);
        runtime.spawn(async move { service.refresh().await });
    }
    Arc::clone(service)
}

/// The process's integrations, if [`open`] ran.
pub fn get() -> Option<&'static Arc<Integrations>> {
    SERVICE.get()
}

impl Integrations {
    /// The built-in integrations under `data_dir`.
    pub fn new(data_dir: &Path) -> Self {
        let dir = data_dir.join(DIR);
        let secrets = secret_store(data_dir, &dir.join(SECRETS_FILE));
        let registry =
            match Registry::new(Arc::clone(&secrets)).with_settings_file(dir.join(SETTINGS_FILE)) {
                Ok(registry) => registry,
                Err(why) => {
                    warn!(%why, "Could not read the integrations' settings; starting without them");
                    Registry::new(secrets)
                }
            };
        let github_app =
            aura_config::CloudConfig::from_env().is_ok_and(|config| config.github_app.is_some());
        Self::with_registry(registry, BuiltinOptions { github_app })
    }

    /// The built-ins added to `registry`.
    pub fn with_registry(mut registry: Registry, options: BuiltinOptions) -> Self {
        for integration in builtin(options) {
            if let Err(why) = registry.add(integration) {
                warn!(%why, "A built-in integration was not added");
            }
        }
        Self {
            registry: Arc::new(registry),
            connected: RwLock::default(),
            github_token: RwLock::default(),
        }
    }

    /// The registry.
    #[must_use]
    pub fn registry(&self) -> &Arc<Registry> {
        &self.registry
    }

    /// Ask every integration whether it is connected, and keep the
    /// answer for kernels and the REPO tab.
    pub async fn refresh(&self) {
        let mut connected = HashSet::new();
        for manifest in self.registry.list() {
            let id = manifest.id.clone();
            if self
                .registry
                .status(id.as_str())
                .await
                .is_ok_and(|status| status.is_connected())
            {
                connected.insert(id);
            }
        }
        info!(connected = connected.len(), "Integrations checked");
        *write(&self.connected) = connected;
        let token = match self.registry.context("github").await {
            Ok(ctx) => ctx.secret().cloned(),
            Err(_) => None,
        };
        *write(&self.github_token) = token;
    }

    /// Register on `executor` the tools agent `agent` (hex id) may use
    /// of the integrations connected at the last refresh.
    pub fn register_tools(&self, executor: &mut ToolExecutor, agent: &str, channel: Option<&str>) {
        let connected = read(&self.connected).clone();
        for tool in self
            .registry
            .tools_where((agent, channel), |id| connected.contains(id))
        {
            executor.register(Box::new(tool));
        }
    }

    /// The GitHub token the Integrations panel stored, if any.
    #[must_use]
    pub fn github_token(&self) -> Option<String> {
        read(&self.github_token)
            .as_ref()
            .map(|secret| secret.expose().to_string())
    }
}

/// The sealed store under the local store's key; in memory when that
/// key can't be had.
fn secret_store(data_dir: &Path, path: &Path) -> Arc<dyn SecretStore> {
    // The local store made its key when it opened; only read it here.
    let key = crate::local_store::KeySource::for_data_dir(data_dir).and_then(|source| {
        source
            .load()?
            .ok_or_else(|| crate::local_store::LocalStoreError::Key {
                location: source.describe(),
                detail: "no key yet".to_string(),
            })
    });
    match key {
        Ok(key) => Arc::new(SealedSecrets::new(path.to_path_buf(), &key)),
        Err(why) => {
            warn!(%why, "No key to seal integration credentials; keeping them for this session only");
            Arc::new(MemorySecrets::default())
        }
    }
}

fn read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests;
