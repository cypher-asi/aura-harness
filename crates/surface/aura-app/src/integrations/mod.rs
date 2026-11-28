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

use aura_agent_kernel::ToolModeClass;
use aura_engine_integrations::builtin::{builtin, BuiltinOptions};
use aura_engine_integrations::{
    tool_name, IntegrationId, MemorySecrets, Registry, SealedSecrets, Secret, SecretStore,
    SECRETS_FILE, SETTINGS_FILE,
};
use aura_exec_tools::{Tool, ToolExecutor};
use aura_model::ToolDefinition;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};
use tracing::{info, warn};

mod ui;
pub(crate) use ui::act;

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
    /// What each integration's last action said, for the panel
    notes: RwLock<std::collections::HashMap<String, (bool, String)>>,
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

/// [`Integrations::tool_classes`] of the process's integrations; empty
/// when [`open`] didn't run.
#[must_use]
pub fn tool_classes() -> BTreeMap<String, ToolModeClass> {
    get().map(|i| i.tool_classes()).unwrap_or_default()
}

/// `tools` with the integration tools agent `agent` may use now
/// appended ([`Integrations::tool_definitions`]), skipping names already
/// there.
#[must_use]
pub fn with_tools_for(mut tools: Vec<ToolDefinition>, agent: &str) -> Vec<ToolDefinition> {
    let Some(integrations) = get() else {
        return tools;
    };
    for definition in integrations.tool_definitions(agent, None) {
        if !tools.iter().any(|tool| tool.name == definition.name) {
            tools.push(definition);
        }
    }
    tools
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
            notes: RwLock::default(),
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

    /// Register on `executor` the tools agent `agent` (hex id) may use,
    /// of every integration, connected or not: an executor lives as long
    /// as its kernel, and an integration connected later must be callable
    /// once its tools are offered ([`Self::tool_definitions`]). Every call
    /// checks connection and access again.
    pub fn register_tools(&self, executor: &mut ToolExecutor, agent: &str, channel: Option<&str>) {
        for tool in self.registry.tools_where((agent, channel), |_| true) {
            executor.register(Box::new(tool));
        }
    }

    /// The model-facing definitions of the tools agent `agent` (hex id)
    /// may use of the integrations connected at the last refresh: what a
    /// turn offers the model, so a newly connected integration shows up
    /// on the next turn.
    #[must_use]
    pub fn tool_definitions(&self, agent: &str, channel: Option<&str>) -> Vec<ToolDefinition> {
        let connected = read(&self.connected).clone();
        self.registry
            .tools_where((agent, channel), |id| connected.contains(id))
            .iter()
            .map(Tool::definition)
            .collect()
    }

    /// The mode class of every integration tool, for the kernel's mode
    /// gate and the approval prompts: `Read` tools are invocations, which
    /// plan and ask mode allow and the `ask` level runs without asking;
    /// the rest are network calls.
    #[must_use]
    pub fn tool_classes(&self) -> BTreeMap<String, ToolModeClass> {
        self.registry
            .list()
            .iter()
            .flat_map(|manifest| {
                manifest.tools.iter().map(|spec| {
                    let class = if spec.effect.is_read() {
                        ToolModeClass::Invoke
                    } else {
                        ToolModeClass::Network
                    };
                    (tool_name(&manifest.id, &spec.name), class)
                })
            })
            .collect()
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
