//! boxd's key for cloud mode: the one stored in the Integrations panel
//! (sealed), else the environment's (`BOXD_TOKEN`, then `BOXD_API_KEY`,
//! from the process or `.env`, as `aura cloud setup` writes it).
//!
//! The stored key wins when both are set: it is the one the user entered
//! last on purpose, and the boxd integration's Test and status already
//! use it first ([`aura_engine_integrations::Registry::context`]). So
//! the key that passes Test is the key cloud mode starts with.
//!
//! The key is the hub's own credential for boxd's API
//! (`CloudHub::from_config`); nothing here sends it to a machine. boxd's
//! manifest says `CloudDelivery::Never`.

use super::Integrations;
use aura_config::cloud::BOXD_API_KEY_PREFIX;
use aura_config::{BoxdApiKey, BoxdCredential, BoxdToken, CloudConfig};
use aura_engine_integrations::Secret;
use std::path::Path;
use tracing::{info, warn};

/// The boxd integration's id.
const BOXD: &str = "boxd";

/// Where cloud mode's boxd key came from. Never shows the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxdKeySource {
    /// The Integrations panel's sealed store
    Integrations,
    /// `BOXD_TOKEN`, from the environment or a `.env` file
    EnvToken,
    /// `BOXD_API_KEY`, from the environment or a `.env` file
    EnvApiKey,
    /// No key anywhere
    None,
}

impl std::fmt::Display for BoxdKeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Integrations => "the Integrations panel (sealed store)",
            Self::EnvToken => "BOXD_TOKEN (environment or .env)",
            Self::EnvApiKey => "BOXD_API_KEY (environment or .env)",
            Self::None => "nowhere",
        })
    }
}

/// `config` with `stored`, boxd's key from the sealed store, in place of
/// its environment credential when there is one; and where the key it
/// ends with came from.
#[must_use]
pub fn with_stored_key(
    mut config: CloudConfig,
    stored: Option<&Secret>,
) -> (CloudConfig, BoxdKeySource) {
    if let Some(stored) = stored.filter(|secret| !secret.is_empty()) {
        config.credential = Some(credential_of(stored));
        return (config, BoxdKeySource::Integrations);
    }
    let source = match &config.credential {
        Some(BoxdCredential::Token(_)) => BoxdKeySource::EnvToken,
        Some(BoxdCredential::ApiKey(_)) => BoxdKeySource::EnvApiKey,
        None => BoxdKeySource::None,
    };
    (config, source)
}

/// A stored key as boxd takes it: an API key (`bxd_…`) is exchanged for
/// a token, anything else is used as a token, as the boxd integration's
/// Test does.
fn credential_of(secret: &Secret) -> BoxdCredential {
    let value = secret.expose().to_string();
    if value.starts_with(BOXD_API_KEY_PREFIX) {
        BoxdCredential::ApiKey(BoxdApiKey::new(value))
    } else {
        BoxdCredential::Token(BoxdToken::new(value))
    }
}

impl Integrations {
    /// boxd's key in the sealed store, read now (blocking file IO). `None`
    /// when there is none or it can't be read (logged).
    #[must_use]
    pub fn stored_boxd_key(&self) -> Option<Secret> {
        match self.registry.stored_secret(BOXD) {
            Ok(stored) => stored,
            Err(why) => {
                warn!(%why, "Could not read boxd's stored key; using the environment's");
                None
            }
        }
    }

    /// `config` with this service's stored boxd key over it, logging
    /// where the key came from (never the key). Blocking file IO.
    #[must_use]
    pub fn cloud_config(&self, config: CloudConfig) -> (CloudConfig, BoxdKeySource) {
        let (config, source) = with_stored_key(config, self.stored_boxd_key().as_ref());
        info!(source = %source, "Cloud mode's boxd key");
        (config, source)
    }
}

/// `config` with the stored boxd key of this process's integrations over
/// it, when they are open; `config` as it is otherwise. Blocking file IO.
#[must_use]
pub fn cloud_config(config: CloudConfig) -> CloudConfig {
    match super::get() {
        Some(service) => service.cloud_config(config).0,
        None => config,
    }
}

/// Whether cloud mode has a key: in the environment, or stored at the
/// last refresh.
#[must_use]
pub fn cloud_configured() -> bool {
    aura_config::cloud().is_configured()
        || super::get().is_some_and(|service| service.has_stored_boxd_key())
}

/// For `aura cloud status` and `setup` outside a session: `config` with
/// the stored boxd key of the data directory `data_dir`, and its source.
/// Blocking file IO.
#[must_use]
pub fn cloud_config_in(data_dir: &Path, config: CloudConfig) -> (CloudConfig, BoxdKeySource) {
    Integrations::new(data_dir).cloud_config(config)
}

#[cfg(test)]
mod tests;
