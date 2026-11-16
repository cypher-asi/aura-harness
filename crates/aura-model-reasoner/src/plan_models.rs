//! Live model lists for linked plans, so the selector shows what the
//! plan can actually use instead of a list baked into the binary.

use std::time::Duration;

use aura_core_auth::{CredentialResolver, ProviderKind};
use serde::Deserialize;

use crate::anthropic::anthropic_api_base;
use crate::model_catalog::{infer_tier, ModelBackend, ModelOption, CLAUDE_PREFIX};
use crate::ReasonerError;

const LIST_TIMEOUT: Duration = Duration::from_secs(5);
/// Guards against a server that keeps reporting `has_more`.
const MAX_PAGES: usize = 10;

#[derive(Deserialize)]
struct ModelPage {
    #[serde(default)]
    data: Vec<ModelEntry>,
    #[serde(default)]
    has_more: bool,
    #[serde(default)]
    last_id: Option<String>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
    #[serde(default)]
    display_name: Option<String>,
}

/// Models the linked Claude plan can call, newest first, as `claude/<id>`
/// selector entries.
///
/// # Errors
///
/// Fails when no Claude plan is linked, or the Models API rejects or
/// cannot be reached.
pub async fn fetch_claude_models(
    resolver: &dyn CredentialResolver,
) -> Result<Vec<ModelOption>, ReasonerError> {
    fetch_claude_models_from(resolver, &anthropic_api_base()).await
}

async fn fetch_claude_models_from(
    resolver: &dyn CredentialResolver,
    base_url: &str,
) -> Result<Vec<ModelOption>, ReasonerError> {
    let credential = resolver
        .inference_credential(ProviderKind::Claude)
        .await
        .map_err(|e| ReasonerError::Api {
            status: 401,
            message: e.to_string(),
        })?;
    let client = reqwest::Client::builder()
        .timeout(LIST_TIMEOUT)
        .build()
        .map_err(|e| ReasonerError::Internal(format!("http client: {e}")))?;
    let base = base_url.trim_end_matches('/');

    let mut options = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut request = client
            .get(format!("{base}/v1/models"))
            .query(&[("limit", "1000")])
            .bearer_auth(&credential.bearer)
            .header("anthropic-version", "2023-06-01");
        if let Some(after) = &after {
            request = request.query(&[("after_id", after.as_str())]);
        }
        for (name, value) in &credential.headers {
            request = request.header(name.as_str(), value.as_str());
        }
        let response = request
            .send()
            .await
            .map_err(|e| ReasonerError::Request(e.without_url().to_string()))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ReasonerError::Api {
                status: status.as_u16(),
                message: format!(
                    "Claude models list: {}",
                    crate::truncate_body(body.trim(), 300)
                ),
            });
        }
        let page: ModelPage = response
            .json()
            .await
            .map_err(|e| ReasonerError::Parse(format!("Claude models list: {e}")))?;
        options.extend(page.data.into_iter().map(|m| {
            let id = format!("{CLAUDE_PREFIX}{}", m.id);
            ModelOption {
                label: m
                    .display_name
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| m.id.clone()),
                tier: infer_tier(&id),
                id,
                backend: ModelBackend::Claude,
                installed: true,
            }
        }));
        match page.last_id {
            Some(last) if page.has_more => after = Some(last),
            _ => break,
        }
    }
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use aura_core_auth::{AuthError, InferenceCredential};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct Linked;

    #[async_trait]
    impl CredentialResolver for Linked {
        async fn inference_credential(
            &self,
            provider: ProviderKind,
        ) -> Result<InferenceCredential, AuthError> {
            Ok(InferenceCredential {
                provider,
                bearer: "plan-token".into(),
                headers: vec![("anthropic-beta".into(), "oauth-2025-04-20".into())],
                system_preamble: None,
            })
        }
    }

    #[tokio::test]
    async fn lists_plan_models_with_the_plan_credential() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = r#"{"data":[
            {"type":"model","id":"claude-opus-5-5","display_name":"Claude Opus 5.5"},
            {"type":"model","id":"claude-sonnet-5","display_name":""}
        ],"has_more":false,"last_id":"claude-sonnet-5"}"#;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let n = socket.read(&mut buf).await.unwrap();
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase()
        });

        let models = fetch_claude_models_from(&Linked, &format!("http://{addr}"))
            .await
            .unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "claude/claude-opus-5-5");
        assert_eq!(models[0].label, "Claude Opus 5.5");
        assert_eq!(models[1].label, "claude-sonnet-5");
        assert_eq!(models[1].backend, ModelBackend::Claude);

        let raw = server.await.unwrap();
        assert!(raw.starts_with("get /v1/models?limit=1000"));
        assert!(raw.contains("authorization: bearer plan-token"));
        assert!(raw.contains("anthropic-beta: oauth-2025-04-20"));
    }
}
