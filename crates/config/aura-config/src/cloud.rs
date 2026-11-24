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

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::env::{
    ConfigError, AURA_CLOUD_FORK_ON_SPAWN, AURA_CLOUD_IDLE_HIBERNATE_SECS,
    AURA_CLOUD_IDLE_STANDBY_SECS, AURA_CLOUD_ISOLATED, AURA_CLOUD_PRICE_DISK_GIB_HOUR,
    AURA_CLOUD_PRICE_MEM_GIB_HOUR, AURA_CLOUD_PRICE_VCPU_HOUR, AURA_CLOUD_SIZE, BOXD_API_KEY,
    BOXD_API_URL, BOXD_BASE_URL, BOXD_ORG, BOXD_REGION, BOXD_TOKEN,
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

/// The last four characters of `secret`, or fewer when it is short.
/// Secrets shorter than 12 characters show nothing at all.
fn tail(secret: &str) -> &str {
    let chars = secret.chars().count();
    if chars < 12 {
        return "";
    }
    let skip = secret
        .char_indices()
        .nth(chars - 4)
        .map_or(secret.len(), |(index, _)| index);
    &secret[skip..]
}

/// A boxd API key (`bxd_` + 43 base62 characters). Exchanged for a
/// short-lived token before each hour of calls.
#[derive(Clone, PartialEq, Eq)]
pub struct BoxdApiKey(Zeroizing<String>);

impl BoxdApiKey {
    /// Wrap a raw key.
    #[must_use]
    pub fn new(raw: impl Into<String>) -> Self {
        Self(Zeroizing::new(raw.into()))
    }

    /// The raw key, for the one call that sends it to boxd.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether the key has boxd's `bxd_` prefix.
    #[must_use]
    pub fn looks_valid(&self) -> bool {
        self.0.starts_with(BOXD_API_KEY_PREFIX)
    }
}

impl fmt::Display for BoxdApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{BOXD_API_KEY_PREFIX}…{}", tail(&self.0))
    }
}

impl fmt::Debug for BoxdApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BoxdApiKey({self})")
    }
}

/// A boxd bearer token the user already holds (a session JWT or a
/// machine token, `bxdm_…`).
#[derive(Clone, PartialEq, Eq)]
pub struct BoxdToken(Zeroizing<String>);

impl BoxdToken {
    /// Wrap a raw token.
    #[must_use]
    pub fn new(raw: impl Into<String>) -> Self {
        Self(Zeroizing::new(raw.into()))
    }

    /// The raw token, for the one call that sends it to boxd.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BoxdToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "…{}", tail(&self.0))
    }
}

impl fmt::Debug for BoxdToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BoxdToken({self})")
    }
}

/// How the app signs in to boxd.
#[derive(Clone, PartialEq, Eq)]
pub enum BoxdCredential {
    /// `BOXD_TOKEN`: used as is until it expires.
    Token(BoxdToken),
    /// `BOXD_API_KEY`: exchanged for a one-hour token, refreshed as needed.
    ApiKey(BoxdApiKey),
}

impl BoxdCredential {
    /// Which variable the credential came from, for status lines.
    #[must_use]
    pub const fn source_var(&self) -> &'static str {
        match self {
            Self::Token(_) => BOXD_TOKEN,
            Self::ApiKey(_) => BOXD_API_KEY,
        }
    }
}

impl fmt::Display for BoxdCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Token(token) => write!(f, "{BOXD_TOKEN} {token}"),
            Self::ApiKey(key) => write!(f, "{BOXD_API_KEY} {key}"),
        }
    }
}

impl fmt::Debug for BoxdCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Token(token) => write!(f, "Token({token:?})"),
            Self::ApiKey(key) => write!(f, "ApiKey({key:?})"),
        }
    }
}

/// The machine sizes boxd offers: vCPUs and memory move together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub enum MachineSize {
    /// 1 vCPU, 4 GiB.
    S1x4,
    /// 2 vCPU, 8 GiB (boxd's own default).
    S2x8,
    /// 4 vCPU, 16 GiB: four parallel Rust compile jobs fit without swap.
    #[default]
    S4x16,
    /// 8 vCPU, 32 GiB, where the org quota allows it.
    S8x32,
}

impl MachineSize {
    /// Every size, smallest first.
    pub const ALL: [Self; 4] = [Self::S1x4, Self::S2x8, Self::S4x16, Self::S8x32];

    /// vCPUs of the size.
    #[must_use]
    pub const fn vcpu(self) -> u32 {
        match self {
            Self::S1x4 => 1,
            Self::S2x8 => 2,
            Self::S4x16 => 4,
            Self::S8x32 => 8,
        }
    }

    /// Memory of the size, in GiB.
    #[must_use]
    pub const fn memory_gib(self) -> u32 {
        self.vcpu() * 4
    }

    /// Memory of the size, in bytes.
    #[must_use]
    pub const fn memory_bytes(self) -> u64 {
        self.memory_gib() as u64 * 1024 * 1024 * 1024
    }

    /// The size as boxd's machine specs name it (`"4x16"`).
    #[must_use]
    pub const fn as_spec(self) -> &'static str {
        match self {
            Self::S1x4 => "1x4",
            Self::S2x8 => "2x8",
            Self::S4x16 => "4x16",
            Self::S8x32 => "8x32",
        }
    }

    /// The next size down, for falling back when a quota refuses this one.
    #[must_use]
    pub const fn smaller(self) -> Option<Self> {
        match self {
            Self::S1x4 => None,
            Self::S2x8 => Some(Self::S1x4),
            Self::S4x16 => Some(Self::S2x8),
            Self::S8x32 => Some(Self::S4x16),
        }
    }

    /// The size with `vcpu` vCPUs, if boxd offers one.
    #[must_use]
    pub fn from_vcpu(vcpu: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|size| size.vcpu() == vcpu)
    }
}

impl fmt::Display for MachineSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_spec())
    }
}

/// Failure to parse a [`MachineSize`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown machine size `{0}`: expected 1x4, 2x8, 4x16 or 8x32")]
pub struct MachineSizeError(pub String);

impl FromStr for MachineSize {
    type Err = MachineSizeError;

    /// Accepts `4x16`, `4x16G`, `4x16GiB` and a bare vCPU count (`4`),
    /// case-insensitively.
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let lower = raw.trim().to_ascii_lowercase();
        let err = || MachineSizeError(raw.trim().to_string());
        let (vcpu, memory) = match lower.split_once('x') {
            Some((vcpu, memory)) => (vcpu, Some(memory)),
            None => (lower.as_str(), None),
        };
        let size = vcpu
            .trim()
            .parse::<u32>()
            .ok()
            .and_then(Self::from_vcpu)
            .ok_or_else(err)?;
        if let Some(memory) = memory {
            let digits = memory
                .trim()
                .trim_end_matches("gib")
                .trim_end_matches("gb")
                .trim_end_matches('g');
            let gib = digits.parse::<u32>().map_err(|_| err())?;
            if gib != size.memory_gib() {
                return Err(err());
            }
        }
        Ok(size)
    }
}

impl Serialize for MachineSize {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_spec())
    }
}

impl<'de> Deserialize<'de> for MachineSize {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
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

        let credential = if let Some(token) = get(BOXD_TOKEN) {
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
        };

        let size = match get(AURA_CLOUD_SIZE) {
            Some(raw) => {
                raw.parse()
                    .map_err(|err: MachineSizeError| ConfigError::InvalidValue {
                        name: AURA_CLOUD_SIZE,
                        raw,
                        message: err.to_string(),
                    })?
            }
            None => defaults.size,
        };

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const KEY: &str = "bxd_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789abcdQ";

    fn config(pairs: &[(&str, &str)]) -> Result<CloudConfig, ConfigError> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        CloudConfig::from_lookup(|name| map.get(name).cloned())
    }

    #[test]
    fn without_a_credential_cloud_is_off() {
        let cfg = config(&[]).expect("defaults parse");
        assert!(!cfg.is_configured());
        assert_eq!(cfg.base_url(), DEFAULT_BOXD_BASE_URL);
        assert_eq!(cfg.size, MachineSize::S4x16);
        assert_eq!(cfg.idle_hibernate_secs, 300);
        assert!(cfg.isolated);
        assert!(cfg.fork_on_spawn);
    }

    #[test]
    fn an_api_key_configures_cloud() {
        let cfg = config(&[(BOXD_API_KEY, KEY)]).expect("parse");
        assert!(cfg.is_configured());
        assert!(matches!(cfg.credential, Some(BoxdCredential::ApiKey(ref k)) if k.expose() == KEY));
    }

    #[test]
    fn a_token_wins_over_a_key() {
        let cfg = config(&[
            (BOXD_API_KEY, KEY),
            (BOXD_TOKEN, "eyJhbGciOiJIUzI1NiJ9.x.y"),
        ])
        .expect("parse");
        assert!(matches!(cfg.credential, Some(BoxdCredential::Token(_))));
    }

    #[test]
    fn blank_values_are_unset() {
        let cfg = config(&[(BOXD_API_KEY, "   "), (BOXD_TOKEN, "")]).expect("parse");
        assert!(!cfg.is_configured());
    }

    #[test]
    fn secrets_never_print() {
        let cfg = config(&[(BOXD_API_KEY, KEY)]).expect("parse");
        let key = BoxdApiKey::new(KEY);
        let token = BoxdToken::new(KEY);
        for text in [
            format!("{key}"),
            format!("{key:?}"),
            format!("{token}"),
            format!("{token:?}"),
            format!("{cfg:?}"),
            format!("{:?}", cfg.credential),
        ] {
            assert!(!text.contains(KEY), "{text}");
            assert!(!text.contains("AbCdEfGh"), "{text}");
        }
        assert_eq!(key.to_string(), "bxd_…bcdQ");
    }

    #[test]
    fn short_secrets_show_no_tail() {
        assert_eq!(BoxdApiKey::new("bxd_abc").to_string(), "bxd_…");
    }

    #[test]
    fn the_old_url_variable_is_read_when_the_new_one_is_unset() {
        let cfg = config(&[(BOXD_API_URL, "https://example:9443")]).expect("parse");
        assert_eq!(cfg.base_url(), "https://example:9443");
        let cfg = config(&[
            (BOXD_API_URL, "https://old:9443"),
            (BOXD_BASE_URL, "https://new:9443"),
        ])
        .expect("parse");
        assert_eq!(cfg.base_url(), "https://new:9443");
    }

    #[test]
    fn machine_sizes_parse_and_print() {
        for size in MachineSize::ALL {
            assert_eq!(size.as_spec().parse::<MachineSize>(), Ok(size));
            assert_eq!(size.to_string(), size.as_spec());
        }
        assert_eq!("8".parse::<MachineSize>(), Ok(MachineSize::S8x32));
        assert_eq!("4X16G".parse::<MachineSize>(), Ok(MachineSize::S4x16));
        assert_eq!("2x8GiB".parse::<MachineSize>(), Ok(MachineSize::S2x8));
        assert!("4x8".parse::<MachineSize>().is_err());
        assert!("3".parse::<MachineSize>().is_err());
        assert_eq!(MachineSize::S8x32.smaller(), Some(MachineSize::S4x16));
        assert_eq!(MachineSize::S1x4.smaller(), None);
        assert_eq!(MachineSize::S4x16.memory_bytes(), 16 * 1024 * 1024 * 1024);
    }

    #[test]
    fn machine_size_serde_uses_the_spec_string() {
        let json = serde_json::to_string(&MachineSize::S8x32).expect("serialize");
        assert_eq!(json, "\"8x32\"");
        let back: MachineSize = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, MachineSize::S8x32);
    }

    #[test]
    fn a_bad_size_names_its_variable() {
        let err = config(&[(AURA_CLOUD_SIZE, "huge")]).expect_err("must fail");
        assert!(err.to_string().contains(AURA_CLOUD_SIZE), "{err}");
    }

    #[test]
    fn a_bad_number_names_its_variable() {
        let err = config(&[(AURA_CLOUD_IDLE_HIBERNATE_SECS, "soon")]).expect_err("must fail");
        assert!(
            err.to_string().contains(AURA_CLOUD_IDLE_HIBERNATE_SECS),
            "{err}"
        );
        let err = config(&[(AURA_CLOUD_PRICE_VCPU_HOUR, "-1")]).expect_err("must fail");
        assert!(
            err.to_string().contains(AURA_CLOUD_PRICE_VCPU_HOUR),
            "{err}"
        );
        let err = config(&[(AURA_CLOUD_ISOLATED, "maybe")]).expect_err("must fail");
        assert!(err.to_string().contains(AURA_CLOUD_ISOLATED), "{err}");
    }

    #[test]
    fn overrides_apply() {
        let cfg = config(&[
            (AURA_CLOUD_SIZE, "8x32"),
            (AURA_CLOUD_IDLE_HIBERNATE_SECS, "60"),
            (AURA_CLOUD_IDLE_STANDBY_SECS, "20"),
            (AURA_CLOUD_ISOLATED, "0"),
            (AURA_CLOUD_FORK_ON_SPAWN, "off"),
            (AURA_CLOUD_PRICE_VCPU_HOUR, "0.05"),
            (BOXD_REGION, "nyc"),
            (BOXD_ORG, "acme"),
        ])
        .expect("parse");
        assert_eq!(cfg.size, MachineSize::S8x32);
        assert_eq!(cfg.idle_hibernate_secs, 60);
        assert_eq!(cfg.idle_standby_secs, 20);
        assert!(!cfg.isolated);
        assert!(!cfg.fork_on_spawn);
        assert_eq!(cfg.price_vcpu_hour, Some(0.05));
        assert_eq!(cfg.region.as_deref(), Some("nyc"));
        assert_eq!(cfg.org.as_deref(), Some("acme"));
    }
}
