//! HTTP client for the reasoner gateway.

use crate::{ProposeRequest, Reasoner, ReasonerConfig};
use async_trait::async_trait;
use aura_core::ProposalSet;
use reqwest::Client;
use std::time::Duration;
use tracing::{debug, error, instrument, warn};

/// HTTP-based reasoner client.
pub struct HttpReasoner {
    client: Client,
    config: ReasonerConfig,
}

impl HttpReasoner {
    /// Create a new HTTP reasoner client.
    ///
    /// # Errors
    /// Returns error if the HTTP client cannot be created.
    pub fn new(config: ReasonerConfig) -> anyhow::Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()?;

        Ok(Self { client, config })
    }

    /// Create with default config.
    ///
    /// # Errors
    /// Returns error if the HTTP client cannot be created.
    pub fn with_defaults() -> anyhow::Result<Self> {
        Self::new(ReasonerConfig::default())
    }
}

#[async_trait]
impl Reasoner for HttpReasoner {
    #[instrument(skip(self, request), fields(agent_id = %request.agent_id))]
    async fn propose(&self, request: ProposeRequest) -> anyhow::Result<ProposalSet> {
        let url = format!("{}/propose", self.config.gateway_url);
        debug!(%url, "Sending propose request");

        let mut last_error = None;

        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                warn!(attempt, "Retrying propose request");
            }

            match self.client.post(&url).json(&request).send().await {
                Ok(response) => {
                    if response.status().is_success() {
                        let proposals: ProposalSet = response.json().await?;
                        debug!(count = proposals.proposals.len(), "Received proposals");
                        return Ok(proposals);
                    }

                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    error!(%status, %body, "Reasoner returned error");
                    last_error = Some(anyhow::anyhow!("Reasoner error: {status} - {body}"));
                }
                Err(e) => {
                    error!(error = %e, "Request failed");
                    last_error = Some(e.into());
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Reasoner request failed")))
    }

    async fn health_check(&self) -> bool {
        let url = format!("{}/health", self.config.gateway_url);

        self.client
            .get(&url)
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reasoner_config_default() {
        let config = ReasonerConfig::default();

        assert!(!config.gateway_url.is_empty());
        assert!(config.timeout_ms > 0);
        // max_retries is u32, so it's always >= 0 by type
        let _ = config.max_retries; // Just access to ensure field exists
    }

    #[test]
    fn test_http_reasoner_new() {
        let config = ReasonerConfig {
            gateway_url: "http://localhost:8080".to_string(),
            timeout_ms: 5000,
            max_retries: 3,
        };

        let reasoner = HttpReasoner::new(config.clone());
        assert!(reasoner.is_ok());

        let reasoner = reasoner.unwrap();
        assert_eq!(reasoner.config.gateway_url, "http://localhost:8080");
        assert_eq!(reasoner.config.timeout_ms, 5000);
        assert_eq!(reasoner.config.max_retries, 3);
    }

    #[test]
    fn test_http_reasoner_with_defaults() {
        let result = HttpReasoner::with_defaults();
        assert!(result.is_ok());
    }

    #[test]
    fn test_http_reasoner_url_building() {
        let config = ReasonerConfig {
            gateway_url: "http://example.com:3000".to_string(),
            timeout_ms: 1000,
            max_retries: 1,
        };

        let reasoner = HttpReasoner::new(config).unwrap();
        
        // Test that the config is stored correctly
        assert_eq!(reasoner.config.gateway_url, "http://example.com:3000");
    }

    #[tokio::test]
    async fn test_health_check_unreachable() {
        // Use a non-routable address to test failure handling
        let config = ReasonerConfig {
            gateway_url: "http://192.0.2.1:1".to_string(), // Non-routable TEST-NET address
            timeout_ms: 100, // Short timeout
            max_retries: 0,
        };

        let reasoner = HttpReasoner::new(config).unwrap();
        let result = reasoner.health_check().await;

        // Should fail because address is unreachable
        assert!(!result);
    }

    // Note: Testing actual HTTP calls would require a mock server.
    // The propose() method cannot be easily unit tested without
    // setting up wiremock or similar HTTP mocking.
    // Integration tests with a real server should cover those cases.
}
