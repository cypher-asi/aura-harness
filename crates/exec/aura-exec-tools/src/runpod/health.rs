//! The Runpod health check: is the key accepted, and is Runpod reachable.

use super::client::{Api, Request};
use super::{RunpodClient, RunpodError};
use reqwest::Method;
use serde_json::Value;

/// What the health check found, for the Integrations panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    /// The key works; the account id and its email, masked.
    Connected {
        account_id: String,
        email: Option<String>,
    },
    /// No key is stored.
    MissingKey,
    /// Runpod rejected the key.
    BadKey,
    /// Runpod's rate limit was hit; the key may be fine.
    RateLimited,
    /// Runpod couldn't be reached, or didn't answer in time.
    Unreachable(String),
}

impl Health {
    /// Whether the integration can be used.
    #[must_use]
    pub const fn is_ok(&self) -> bool {
        matches!(self, Self::Connected { .. })
    }
}

/// Check the key against `GET /v2/account`. Never fails: every outcome is
/// a [`Health`].
pub async fn health_check(client: &RunpodClient) -> Health {
    let request = Request::new(
        "check the Runpod connection",
        Method::GET,
        Api::Control,
        "/v2/account".to_string(),
    );
    match client.json(request).await {
        Ok(account) => {
            let field = |name: &str| {
                account
                    .get(name)
                    .and_then(Value::as_str)
                    .map(str::to_string)
            };
            Health::Connected {
                account_id: field("id").unwrap_or_default(),
                email: field("email").map(|e| mask_email(&e)),
            }
        }
        Err(RunpodError::MissingKey { .. }) => Health::MissingKey,
        Err(RunpodError::Unauthorized { .. }) => Health::BadKey,
        Err(RunpodError::RateLimited { .. }) => Health::RateLimited,
        Err(other) => Health::Unreachable(other.to_string()),
    }
}

/// `d***@example.com`: enough to recognize the account, not to copy.
fn mask_email(email: &str) -> String {
    match email.split_once('@') {
        Some((user, domain)) => {
            let first: String = user.chars().take(1).collect();
            format!("{first}***@{domain}")
        }
        None => "***".to_string(),
    }
}
