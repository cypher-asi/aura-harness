//! The Runpod API key and where it is read from.

use std::fmt;

/// The variable Runpod's own CLI and SDK read the key from.
pub const RUNPOD_API_KEY_ENV: &str = "RUNPOD_API_KEY";

/// A Runpod API key. Never printed: `Debug` and `Display` show
/// `ApiKey(***)`, and the only way to the raw value is the crate-private
/// [`ApiKey::expose`] the client uses for the `Authorization` header.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// A key from `raw`, trimmed; `None` when it is empty.
    #[must_use]
    pub fn new(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        (!raw.is_empty()).then(|| Self(raw.to_string()))
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(***)")
    }
}

impl fmt::Display for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(***)")
    }
}

/// Where the key is read from, at call time, so a key added or changed
/// in the Integrations panel is used without a restart.
pub trait ApiKeySource: Send + Sync {
    /// The key, or `None` when none is stored.
    fn api_key(&self) -> Option<ApiKey>;

    /// Where the key is looked for, for error messages. Never the key.
    fn describe(&self) -> String;
}

/// The key in `RUNPOD_API_KEY`.
#[derive(Debug, Clone, Copy, Default)]
pub struct EnvApiKey;

impl ApiKeySource for EnvApiKey {
    fn api_key(&self) -> Option<ApiKey> {
        std::env::var(RUNPOD_API_KEY_ENV)
            .ok()
            .and_then(|v| ApiKey::new(&v))
    }

    fn describe(&self) -> String {
        format!("the {RUNPOD_API_KEY_ENV} environment variable")
    }
}

/// A key fixed when built: the integration model hands one over after
/// reading its secret storage, and tests use it.
#[derive(Debug, Clone)]
pub struct StaticApiKey(pub Option<ApiKey>);

impl ApiKeySource for StaticApiKey {
    fn api_key(&self) -> Option<ApiKey> {
        self.0.clone()
    }

    fn describe(&self) -> String {
        "the Runpod API key in Integrations".to_string()
    }
}
