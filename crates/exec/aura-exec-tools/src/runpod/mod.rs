//! Runpod: agent tools over the Runpod REST API v2 (pods, the GPU
//! catalog, billing) and the serverless job API.
//!
//! # Boundaries
//!
//! * **Key.** One API key, read at call time from an [`ApiKeySource`]
//!   (the integration model's secret storage, or `RUNPOD_API_KEY`). It is
//!   only ever sent as the `Authorization: Bearer` header: [`ApiKey`]
//!   prints as `ApiKey(***)`, and no error or tool output carries it.
//! * **Timeouts.** Every request has a connect and a total timeout
//!   ([`RunpodSettings`]); a log stream is read for a bounded time.
//! * **Output.** Responses are read up to a byte cap, lists keep only the
//!   fields an agent needs, and every tool result is capped at
//!   [`output::MAX_OUTPUT_BYTES`].
//! * **Spending.** Calls that spend money or destroy resources are bound
//!   by [`SpendLimits`], checked inside the tools before the request is
//!   sent, so they hold at every approval level, `yolo` included.

mod client;
mod error;
mod health;
mod key;
mod limits;
mod output;
mod tools;

#[cfg(test)]
mod tests;

pub use client::RunpodClient;
pub use error::RunpodError;
pub use health::{health_check, Health};
pub use key::{ApiKey, ApiKeySource, EnvApiKey, StaticApiKey, RUNPOD_API_KEY_ENV};
pub use limits::SpendLimits;
pub use output::MAX_OUTPUT_BYTES;
pub use tools::{runpod_tools, RunpodOp, RunpodTool, ToolKind};

/// `value` when it fits in [`MAX_OUTPUT_BYTES`], else its rendering cut
/// with a note: what a caller returning JSON hands on.
#[must_use]
pub fn bounded(value: serde_json::Value) -> serde_json::Value {
    output::bounded(value)
}

use std::time::Duration;

/// Default control-plane base URL (`/v2/pods`, `/v2/catalog`, ...).
pub const DEFAULT_API_BASE: &str = "https://api.runpod.io";

/// Default serverless job base URL (`/v2/{endpoint}/run`, ...).
pub const DEFAULT_SERVERLESS_BASE: &str = "https://api.runpod.ai";

/// Where the tools send requests, how long they wait, and the spending
/// limits. Non-secret: the key comes from an [`ApiKeySource`].
#[derive(Debug, Clone, PartialEq)]
pub struct RunpodSettings {
    /// Control-plane base URL, without a trailing slash.
    pub api_base: String,
    /// Serverless base URL, without a trailing slash.
    pub serverless_base: String,
    /// Total time for one control-plane request, body included.
    pub request_timeout: Duration,
    /// Time to open a connection.
    pub connect_timeout: Duration,
    /// Longest a `runpod_run_job` call with `wait` blocks for the result.
    pub job_wait: Duration,
    /// How long `runpod_pod_logs` reads the log stream.
    pub log_read: Duration,
    /// Limits on calls that spend money.
    pub limits: SpendLimits,
}

impl Default for RunpodSettings {
    fn default() -> Self {
        Self {
            api_base: DEFAULT_API_BASE.to_string(),
            serverless_base: DEFAULT_SERVERLESS_BASE.to_string(),
            request_timeout: Duration::from_secs(20),
            connect_timeout: Duration::from_secs(5),
            job_wait: Duration::from_secs(60),
            log_read: Duration::from_secs(10),
            limits: SpendLimits::default(),
        }
    }
}

impl RunpodSettings {
    /// Settings from config values looked up by name: `api_base`,
    /// `serverless_base` and the [`SpendLimits::from_lookup`] fields.
    /// Missing or unreadable values keep their defaults.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let base = |name: &str, default: &str| {
            get(name)
                .map(|v| v.trim().trim_end_matches('/').to_string())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| default.to_string())
        };
        Self {
            api_base: base("api_base", DEFAULT_API_BASE),
            serverless_base: base("serverless_base", DEFAULT_SERVERLESS_BASE),
            limits: SpendLimits::from_lookup(&get),
            ..Self::default()
        }
    }
}
