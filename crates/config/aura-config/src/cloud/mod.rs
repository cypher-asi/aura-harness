//! Cloud agents on boxd machines: credentials, machine size, sleep timing
//! and price overrides.
//!
//! Every value comes from the process environment, which a `.env` file
//! can set (see [`crate::env_file`]). `BOXD_*` are the names boxd's own
//! SDKs read, in the same order (a token wins over an API key), so one
//! `.env` serves both. [`CloudConfig::from_env`] is the only reader of
//! these variables.
//!
//! ## Invariants
//!
//! - Secrets ([`BoxdApiKey`], [`BoxdToken`]) are zeroized on drop and
//!   never printed: `Debug` and `Display` show a redacted form
//!   (`bxd_…a1B2`), and they are not `Serialize`. Code that sends one to
//!   boxd calls `expose()` at that single point.
//! - An empty or blank variable counts as unset.

mod credential;
mod machine_size;
#[cfg(test)]
mod tests;

use std::path::PathBuf;

use crate::env::{
    ConfigError, AURA_CLOUD_BINARY, AURA_CLOUD_FORK_ON_SPAWN, AURA_CLOUD_IDLE_HIBERNATE_SECS,
    AURA_CLOUD_IDLE_STANDBY_SECS, AURA_CLOUD_ISOLATED, AURA_CLOUD_PRICE_DISK_GIB_HOUR,
    AURA_CLOUD_PRICE_MEM_GIB_HOUR, AURA_CLOUD_PRICE_VCPU_HOUR, AURA_CLOUD_SIZE, AURA_GITHUB_APP_ID,
    AURA_GITHUB_APP_PRIVATE_KEY_PATH, AURA_GITHUB_APP_WORKFLOWS, BOXD_API_KEY, BOXD_API_URL,
    BOXD_BASE_URL, BOXD_ORG, BOXD_REGION, BOXD_TOKEN,
};

/// Shown when cloud mode is asked for without a credential.
pub const MISSING_CREDENTIAL_NOTICE: &str =
    "Cloud needs BOXD_API_KEY: run aura cloud setup, or add it to .env or ~/.aura/.env";

/// boxd's public API endpoint (Frankfurt, the home region).
pub const DEFAULT_BOXD_BASE_URL: &str = "https://boxd.sh:9443";

/// Prefix of every boxd API key.
pub const BOXD_API_KEY_PREFIX: &str = "bxd_";

/// Idle seconds before a cloud machine hibernates, by default.
pub const DEFAULT_IDLE_HIBERNATE_SECS: u64 = 300;

const TRUTHY: &[&str] = &["1", "true", "yes", "on"];
const FALSY: &[&str] = &["0", "false", "no", "off"];

pub use credential::{BoxdApiKey, BoxdCredential, BoxdToken};
pub use machine_size::{MachineSize, MachineSizeError};

/// The GitHub App cloud machines push as. The hub signs its App JWTs
/// with the private key and mints each machine a short-lived token for
/// one repository; the key itself never leaves the hub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubAppConfig {
    /// The App's client id (`Iv23…`) or numeric app id
    /// (`AURA_GITHUB_APP_ID`): the App JWT's issuer.
    pub app_id: String,
    /// PEM file with its private key
    /// (`AURA_GITHUB_APP_PRIVATE_KEY_PATH`). Read when a token is
    /// minted, so a rotated key needs no restart.
    pub private_key_path: PathBuf,
    /// Whether repo tokens may also edit workflows
    /// (`AURA_GITHUB_APP_WORKFLOWS`). Off by default: a workflow edit
    /// can read the repository's Actions secrets.
    pub workflows: bool,
}

/// Settings for running agents on boxd machines.
#[derive(Debug, Clone, PartialEq)]
pub struct CloudConfig {
    /// `BOXD_TOKEN`, else `BOXD_API_KEY`. `None` means cloud mode is off.
    pub credential: Option<BoxdCredential>,
    /// gRPC endpoint (`BOXD_BASE_URL`, or the older `BOXD_API_URL`);
    /// `None` = [`DEFAULT_BOXD_BASE_URL`]. Read it with [`Self::base_url`].
    pub base_url: Option<String>,
    /// Region for new machines (`BOXD_REGION`); `None` = the endpoint's.
    pub region: Option<String>,
    /// Organization to act in (`BOXD_ORG`); `None` = the key's own.
    pub org: Option<String>,
    /// Size of new machines (`AURA_CLOUD_SIZE`).
    pub size: MachineSize,
    /// Idle seconds before a machine hibernates
    /// (`AURA_CLOUD_IDLE_HIBERNATE_SECS`); `0` never hibernates it.
    pub idle_hibernate_secs: u64,
    /// Idle seconds before a machine goes to standby first
    /// (`AURA_CLOUD_IDLE_STANDBY_SECS`); `0` skips standby.
    pub idle_standby_secs: u64,
    /// Whether machines are created isolated (`AURA_CLOUD_ISOLATED`).
    pub isolated: bool,
    /// Whether a spawned child of a cloud agent gets a fork of its
    /// parent's machine (`AURA_CLOUD_FORK_ON_SPAWN`).
    pub fork_on_spawn: bool,
    /// Override of the price of an active vCPU-hour.
    pub price_vcpu_hour: Option<f64>,
    /// Override of the price of a GiB-hour of memory in use.
    pub price_mem_gib_hour: Option<f64>,
    /// Override of the price of a GiB-hour of disk written.
    pub price_disk_gib_hour: Option<f64>,
    /// Linux aura build for the base snapshot (`AURA_CLOUD_BINARY`);
    /// `None` lets the hub pick one.
    pub binary: Option<PathBuf>,
    /// The GitHub App cloud machines push as; `None` (either variable
    /// unset) keeps cloud agents' commits on their machines until a
    /// pull brings them back.
    pub github_app: Option<GithubAppConfig>,
}

impl CloudConfig {
    /// Compile-time defaults: no credential, so cloud mode is off.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            credential: None,
            base_url: None,
            region: None,
            org: None,
            size: MachineSize::S4x16,
            idle_hibernate_secs: DEFAULT_IDLE_HIBERNATE_SECS,
            idle_standby_secs: 0,
            isolated: true,
            fork_on_spawn: true,
            price_vcpu_hour: None,
            price_mem_gib_hour: None,
            price_disk_gib_hour: None,
            binary: None,
            github_app: None,
        }
    }

    /// Read the process environment.
    ///
    /// # Errors
    ///
    /// [`ConfigError::InvalidValue`] naming the variable when a size,
    /// number or switch does not parse.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Parse `.env` files again (picking up a key added after start-up)
    /// and then read the process environment. Does blocking file IO.
    ///
    /// # Errors
    ///
    /// As [`Self::from_env`].
    pub fn reload() -> Result<Self, ConfigError> {
        let _ = crate::env_file::reload();
        Self::from_env()
    }

    /// Read settings through `lookup`, so tests need no process env.
    ///
    /// # Errors
    ///
    /// As [`Self::from_env`].
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let get = |name: &str| {
            lookup(name)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let defaults = Self::defaults();
        let credential = parse_credential(&get);
        let size = parse_size(&get, defaults.size)?;
        let github_app = parse_github_app(&get)?;
        Ok(Self {
            credential,
            base_url: get(BOXD_BASE_URL).or_else(|| get(BOXD_API_URL)),
            region: get(BOXD_REGION),
            org: get(BOXD_ORG),
            size,
            idle_hibernate_secs: parse_number(&get, AURA_CLOUD_IDLE_HIBERNATE_SECS)?
                .unwrap_or(defaults.idle_hibernate_secs),
            idle_standby_secs: parse_number(&get, AURA_CLOUD_IDLE_STANDBY_SECS)?
                .unwrap_or(defaults.idle_standby_secs),
            isolated: parse_switch(&get, AURA_CLOUD_ISOLATED, defaults.isolated)?,
            fork_on_spawn: parse_switch(&get, AURA_CLOUD_FORK_ON_SPAWN, defaults.fork_on_spawn)?,
            price_vcpu_hour: parse_price(&get, AURA_CLOUD_PRICE_VCPU_HOUR)?,
            price_mem_gib_hour: parse_price(&get, AURA_CLOUD_PRICE_MEM_GIB_HOUR)?,
            price_disk_gib_hour: parse_price(&get, AURA_CLOUD_PRICE_DISK_GIB_HOUR)?,
            binary: get(AURA_CLOUD_BINARY).map(PathBuf::from),
            github_app,
        })
    }

    /// The gRPC endpoint to call.
    #[must_use]
    pub fn base_url(&self) -> &str {
        self.base_url.as_deref().unwrap_or(DEFAULT_BOXD_BASE_URL)
    }

    /// Whether cloud mode can be used: a credential is set.
    #[must_use]
    pub const fn is_configured(&self) -> bool {
        self.credential.is_some()
    }
}

impl Default for CloudConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

/// `BOXD_TOKEN`, else `BOXD_API_KEY` (warning when it lacks the key
/// prefix), else `None`.
fn parse_credential(get: &impl Fn(&str) -> Option<String>) -> Option<BoxdCredential> {
    if let Some(token) = get(BOXD_TOKEN) {
        Some(BoxdCredential::Token(BoxdToken::new(token)))
    } else if let Some(key) = get(BOXD_API_KEY) {
        let key = BoxdApiKey::new(key);
        if !key.looks_valid() {
            tracing::warn!(
                "{BOXD_API_KEY} does not start with `{BOXD_API_KEY_PREFIX}`; boxd may refuse it"
            );
        }
        Some(BoxdCredential::ApiKey(key))
    } else {
        None
    }
}

fn parse_size(
    get: &impl Fn(&str) -> Option<String>,
    default: MachineSize,
) -> Result<MachineSize, ConfigError> {
    match get(AURA_CLOUD_SIZE) {
        Some(raw) => raw
            .parse()
            .map_err(|err: MachineSizeError| ConfigError::InvalidValue {
                name: AURA_CLOUD_SIZE,
                raw,
                message: err.to_string(),
            }),
        None => Ok(default),
    }
}

/// The GitHub App when both its variables are set; a warning and `None`
/// when only one is.
fn parse_github_app(
    get: &impl Fn(&str) -> Option<String>,
) -> Result<Option<GithubAppConfig>, ConfigError> {
    let workflows = parse_switch(get, AURA_GITHUB_APP_WORKFLOWS, false)?;
    let github_app = match (
        get(AURA_GITHUB_APP_ID),
        get(AURA_GITHUB_APP_PRIVATE_KEY_PATH),
    ) {
        (Some(app_id), Some(path)) => Some(GithubAppConfig {
            app_id,
            private_key_path: expand_home(&path, dirs::home_dir()),
            workflows,
        }),
        (None, None) => None,
        (Some(_), None) | (None, Some(_)) => {
            tracing::warn!(
                "the GitHub App needs both {AURA_GITHUB_APP_ID} and {AURA_GITHUB_APP_PRIVATE_KEY_PATH}; cloud agents won't push"
            );
            None
        }
    };
    Ok(github_app)
}

/// `raw` with a leading `~/` replaced by `home`, as a shell would.
fn expand_home(raw: &str, home: Option<PathBuf>) -> PathBuf {
    match (raw.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(raw),
    }
}

fn parse_number(
    get: &impl Fn(&str) -> Option<String>,
    name: &'static str,
) -> Result<Option<u64>, ConfigError> {
    get(name)
        .map(|raw| {
            raw.parse::<u64>().map_err(|err| ConfigError::InvalidValue {
                name,
                message: err.to_string(),
                raw,
            })
        })
        .transpose()
}

fn parse_price(
    get: &impl Fn(&str) -> Option<String>,
    name: &'static str,
) -> Result<Option<f64>, ConfigError> {
    get(name)
        .map(|raw| match raw.parse::<f64>() {
            Ok(value) if value.is_finite() && value >= 0.0 => Ok(value),
            Ok(_) => Err(ConfigError::InvalidValue {
                name,
                raw,
                message: "must be a finite, non-negative number".to_string(),
            }),
            Err(err) => Err(ConfigError::InvalidValue {
                name,
                message: err.to_string(),
                raw,
            }),
        })
        .transpose()
}

fn parse_switch(
    get: &impl Fn(&str) -> Option<String>,
    name: &'static str,
    default: bool,
) -> Result<bool, ConfigError> {
    let Some(raw) = get(name) else {
        return Ok(default);
    };
    let lower = raw.to_ascii_lowercase();
    if TRUTHY.contains(&lower.as_str()) {
        Ok(true)
    } else if FALSY.contains(&lower.as_str()) {
        Ok(false)
    } else {
        Err(ConfigError::InvalidValue {
            name,
            raw,
            message: "expected 1/0, true/false, yes/no or on/off".to_string(),
        })
    }
}
