//! What an integration gets for one operation: its credential and
//! config, a shared HTTP client and the time it has.

use crate::error::IntegrationError;
use crate::id::IntegrationId;
use crate::secret::Secret;
use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;

/// Longest service error message kept in an [`IntegrationError::Http`].
const MAX_MESSAGE: usize = 200;

/// Where the credential in a context came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretSource {
    /// Stored through the registry (sealed)
    Stored,
    /// This environment variable
    Env(String),
    /// Given to [`crate::Registry::connect`], not stored yet
    Candidate,
}

/// One operation's view of one integration.
#[derive(Debug, Clone)]
pub struct IntegrationContext {
    id: IntegrationId,
    secret: Option<(Secret, SecretSource)>,
    config: BTreeMap<String, String>,
    http: reqwest::Client,
    timeout: Duration,
}

impl IntegrationContext {
    /// A context for `id` with no credential or config, timing out after
    /// `timeout`. The registry builds them; tests may too.
    #[must_use]
    pub fn new(id: IntegrationId, http: reqwest::Client, timeout: Duration) -> Self {
        Self {
            id,
            secret: None,
            config: BTreeMap::new(),
            http,
            timeout,
        }
    }

    /// This context with `secret`, from `source`.
    #[must_use]
    pub fn with_secret(mut self, secret: Secret, source: SecretSource) -> Self {
        self.secret = Some((secret, source));
        self
    }

    /// This context with `config`.
    #[must_use]
    pub fn with_config(mut self, config: BTreeMap<String, String>) -> Self {
        self.config = config;
        self
    }

    /// This context timing out after `timeout`.
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The integration.
    #[must_use]
    pub const fn id(&self) -> &IntegrationId {
        &self.id
    }

    /// Its credential: the stored one, else the first of its manifest's
    /// env variables set.
    #[must_use]
    pub fn secret(&self) -> Option<&Secret> {
        self.secret.as_ref().map(|(secret, _)| secret)
    }

    /// Where [`Self::secret`] came from.
    #[must_use]
    pub fn secret_source(&self) -> Option<&SecretSource> {
        self.secret.as_ref().map(|(_, source)| source)
    }

    /// The credential, or [`IntegrationError::NotConnected`] with `hint`.
    ///
    /// # Errors
    /// There is none.
    pub fn require_secret(&self, hint: &str) -> Result<&Secret, IntegrationError> {
        self.secret().ok_or_else(|| IntegrationError::NotConnected {
            id: self.id.clone(),
            hint: hint.to_string(),
        })
    }

    /// Config value `key`, as set or the field's default.
    #[must_use]
    pub fn config(&self, key: &str) -> Option<&str> {
        self.config.get(key).map(String::as_str)
    }

    /// The shared HTTP client. Send through [`Self::send`] for the
    /// timeout and errors that carry no secret.
    #[must_use]
    pub const fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// How long one call may take.
    #[must_use]
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Run `work` within [`Self::timeout`].
    ///
    /// # Errors
    /// [`IntegrationError::Timeout`] when it runs past it, else `work`'s.
    pub async fn timed<T>(
        &self,
        work: impl Future<Output = Result<T, IntegrationError>>,
    ) -> Result<T, IntegrationError> {
        tokio::time::timeout(self.timeout, work)
            .await
            .map_err(|_| IntegrationError::Timeout {
                id: self.id.clone(),
                after: self.timeout,
            })?
    }

    /// Send `request` within [`Self::timeout`] and read its JSON. A
    /// status other than 2xx is [`IntegrationError::Http`] with the
    /// service's `message` or `error` field only.
    ///
    /// # Errors
    /// It timed out, couldn't be sent, answered an error, or its body is
    /// not JSON.
    pub async fn send(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<serde_json::Value, IntegrationError> {
        self.timed(async {
            let response = request
                .timeout(self.timeout)
                .send()
                .await
                .map_err(|e| self.network(e))?;
            let status = response.status();
            let body = response.text().await.map_err(|e| self.network(e))?;
            let json: serde_json::Value =
                serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
            if status.is_success() {
                return Ok(json);
            }
            Err(IntegrationError::Http {
                id: self.id.clone(),
                status: status.as_u16(),
                message: service_message(&json, status),
            })
        })
        .await
    }

    fn network(&self, error: reqwest::Error) -> IntegrationError {
        if error.is_timeout() {
            return IntegrationError::Timeout {
                id: self.id.clone(),
                after: self.timeout,
            };
        }
        // Without the URL: some services take keys in query strings.
        IntegrationError::Network {
            id: self.id.clone(),
            message: error.without_url().to_string(),
        }
    }
}

/// The service's own error message in `json`, cut short; else the
/// status's reason.
fn service_message(json: &serde_json::Value, status: reqwest::StatusCode) -> String {
    let said = ["message", "error", "error_description"]
        .iter()
        .find_map(|key| json.get(key).and_then(serde_json::Value::as_str));
    let said = said.unwrap_or_else(|| status.canonical_reason().unwrap_or("error"));
    said.chars().take(MAX_MESSAGE).collect()
}
