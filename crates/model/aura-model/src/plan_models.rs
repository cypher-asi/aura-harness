//! Live model lists for linked plans, so the selector shows what the
//! plan can actually use instead of a list baked into the binary.

use std::time::Duration;

use aura_core_auth::{CredentialResolver, ProviderKind};
use serde::Deserialize;

use crate::anthropic::anthropic_api_base;
use crate::model_catalog::{
    infer_tier, LiveModelInfo, LivePrice, ModelBackend, ModelOption, CLAUDE_PREFIX,
    OPENROUTER_PREFIX,
};
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

#[derive(Deserialize)]
struct OpenRouterList {
    #[serde(default)]
    data: Vec<OpenRouterModel>,
}

#[derive(Deserialize)]
struct OpenRouterModel {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    context_length: Option<u64>,
    #[serde(default)]
    pricing: Option<OpenRouterPricing>,
    #[serde(default)]
    top_provider: Option<OpenRouterTopProvider>,
    #[serde(default)]
    supported_parameters: Vec<String>,
}

/// USD per token, as decimal strings; `"-1"` means variable pricing.
#[derive(Deserialize)]
struct OpenRouterPricing {
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    completion: Option<String>,
    #[serde(default)]
    input_cache_read: Option<String>,
    #[serde(default)]
    input_cache_write: Option<String>,
}

#[derive(Deserialize)]
struct OpenRouterTopProvider {
    #[serde(default)]
    max_completion_tokens: Option<u64>,
}

/// Models the user's OpenRouter key can call with tools, as
/// `openrouter/<slug>` selector entries, with the limits and prices
/// OpenRouter lists for them.
///
/// # Errors
///
/// Fails when no OpenRouter key is linked, or the models list rejects or
/// cannot be reached.
pub async fn fetch_openrouter_models(
    resolver: &dyn CredentialResolver,
) -> Result<Vec<(ModelOption, LiveModelInfo)>, ReasonerError> {
    fetch_openrouter_models_from(resolver, &crate::anthropic::openrouter_api_base()).await
}

async fn fetch_openrouter_models_from(
    resolver: &dyn CredentialResolver,
    base_url: &str,
) -> Result<Vec<(ModelOption, LiveModelInfo)>, ReasonerError> {
    let credential = resolver
        .inference_credential(ProviderKind::OpenRouter)
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

    let mut request = client
        .get(format!("{base}/v1/models"))
        .bearer_auth(&credential.bearer);
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
                "OpenRouter models list: {}",
                crate::truncate_body(body.trim(), 300)
            ),
        });
    }
    let list: OpenRouterList = response
        .json()
        .await
        .map_err(|e| ReasonerError::Parse(format!("OpenRouter models list: {e}")))?;
    Ok(list
        .data
        .into_iter()
        .filter(|m| m.supported_parameters.iter().any(|p| p == "tools"))
        .map(openrouter_entry)
        .collect())
}

fn openrouter_entry(model: OpenRouterModel) -> (ModelOption, LiveModelInfo) {
    let id = format!("{OPENROUTER_PREFIX}{}", model.id);
    let info = LiveModelInfo {
        context_window: model.context_length,
        max_output: model
            .top_provider
            .and_then(|p| p.max_completion_tokens)
            .map(|tokens| u32::try_from(tokens).unwrap_or(u32::MAX)),
        price: model.pricing.as_ref().and_then(openrouter_price),
    };
    let option = ModelOption {
        label: model.name.filter(|n| !n.is_empty()).unwrap_or(model.id),
        tier: infer_tier(&id),
        id,
        backend: ModelBackend::OpenRouter,
        installed: true,
    };
    (option, info)
}

/// Per-million price, or `None` unless both prompt and completion are
/// known; missing cache prices fall back to the prompt price.
fn openrouter_price(pricing: &OpenRouterPricing) -> Option<LivePrice> {
    let input = per_million(pricing.prompt.as_deref())?;
    let output = per_million(pricing.completion.as_deref())?;
    Some(LivePrice {
        input,
        output,
        cache_write: per_million(pricing.input_cache_write.as_deref()).unwrap_or(input),
        cache_read: per_million(pricing.input_cache_read.as_deref()).unwrap_or(input),
    })
}

/// A USD-per-token string as USD per million tokens; negative (variable)
/// or unparseable prices are unknown.
fn per_million(per_token: Option<&str>) -> Option<f64> {
    per_token?
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map(|v| v * 1_000_000.0)
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

    #[tokio::test]
    async fn lists_tool_capable_openrouter_models_with_limits_and_prices() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = r#"{"data":[
            {"id":"anthropic/claude-sonnet-4.6","name":"Anthropic: Claude Sonnet 4.6",
             "context_length":200000,
             "pricing":{"prompt":"0.000003","completion":"0.000015",
                        "input_cache_read":"0.0000003","input_cache_write":"0.00000375"},
             "top_provider":{"max_completion_tokens":64000},
             "supported_parameters":["tools","reasoning"]},
            {"id":"some/no-tools","name":"No Tools","supported_parameters":["temperature"]},
            {"id":"meta-llama/llama-4:free","pricing":{"prompt":"-1","completion":"0"},
             "supported_parameters":["tools"]}
        ]}"#;
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

        let models = fetch_openrouter_models_from(&Linked, &format!("http://{addr}/"))
            .await
            .unwrap();
        assert_eq!(models.len(), 2);
        let (option, info) = &models[0];
        assert_eq!(option.id, "openrouter/anthropic/claude-sonnet-4.6");
        assert_eq!(option.label, "Anthropic: Claude Sonnet 4.6");
        assert_eq!(option.backend, ModelBackend::OpenRouter);
        assert!(option.installed);
        assert_eq!(info.context_window, Some(200_000));
        assert_eq!(info.max_output, Some(64_000));
        let price = info.price.unwrap();
        assert!((price.input - 3.0).abs() < 1e-9);
        assert!((price.output - 15.0).abs() < 1e-9);
        assert!((price.cache_read - 0.3).abs() < 1e-9);
        assert!((price.cache_write - 3.75).abs() < 1e-9);

        let (free, free_info) = &models[1];
        assert_eq!(free.id, "openrouter/meta-llama/llama-4:free");
        assert_eq!(free.label, "meta-llama/llama-4:free");
        assert_eq!(free_info.context_window, None);
        assert_eq!(free_info.max_output, None);
        assert_eq!(free_info.price, None);

        let raw = server.await.unwrap();
        assert!(raw.starts_with("get /v1/models "));
        assert!(raw.contains("authorization: bearer plan-token"));
        assert!(raw.contains("anthropic-beta: oauth-2025-04-20"));
    }

    #[test]
    fn openrouter_cache_prices_default_to_the_prompt_price() {
        let pricing = OpenRouterPricing {
            prompt: Some("0.000002".into()),
            completion: Some("0.000008".into()),
            input_cache_read: None,
            input_cache_write: Some("garbage".into()),
        };
        let price = openrouter_price(&pricing).unwrap();
        assert!((price.cache_read - 2.0).abs() < 1e-9);
        assert!((price.cache_write - 2.0).abs() < 1e-9);
    }
}
