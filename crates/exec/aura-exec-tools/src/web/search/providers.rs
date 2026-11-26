//! The search backends: Brave Search, Tavily and SearXNG.

use super::super::html::html_to_text;
use super::super::{api_client, read_capped, validate_url, WebError};
use super::{SearchHit, SearchProvider};
use async_trait::async_trait;
use serde::Deserialize;

/// Largest provider response read.
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

const BRAVE_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";
const TAVILY_ENDPOINT: &str = "https://api.tavily.com/search";

/// Brave Search API (`X-Subscription-Token`).
pub struct BraveSearch {
    pub(super) client: reqwest::Client,
    pub(super) api_key: String,
    pub(super) endpoint: String,
}

impl BraveSearch {
    /// # Errors
    /// Fails when the HTTP client cannot be created.
    pub fn new(api_key: impl Into<String>) -> Result<Self, WebError> {
        Ok(Self {
            client: api_client()?,
            api_key: api_key.into(),
            endpoint: BRAVE_ENDPOINT.into(),
        })
    }

    /// Point at another endpoint (tests, proxies).
    #[must_use]
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }
}

#[async_trait]
impl SearchProvider for BraveSearch {
    fn name(&self) -> &'static str {
        "Brave Search"
    }

    async fn search(&self, query: &str, count: usize) -> Result<Vec<SearchHit>, WebError> {
        let count = count.to_string();
        let request = self
            .client
            .get(&self.endpoint)
            .query(&[("q", query), ("count", count.as_str())])
            .header(reqwest::header::ACCEPT, "application/json")
            .header("X-Subscription-Token", &self.api_key);
        let body: BraveResponse = send_json(self.name(), &self.endpoint, request).await?;
        Ok(body
            .web
            .map(|w| w.results)
            .unwrap_or_default()
            .into_iter()
            .map(|r| hit(&r.title, r.url, &r.description))
            .collect())
    }
}

#[derive(Deserialize)]
struct BraveResponse {
    #[serde(default)]
    web: Option<BraveWeb>,
}

#[derive(Deserialize)]
struct BraveWeb {
    #[serde(default)]
    results: Vec<BraveResult>,
}

#[derive(Deserialize)]
struct BraveResult {
    #[serde(default)]
    title: String,
    url: String,
    #[serde(default)]
    description: String,
}

/// Tavily search API (bearer token).
pub struct TavilySearch {
    client: reqwest::Client,
    api_key: String,
    endpoint: String,
}

impl TavilySearch {
    /// # Errors
    /// Fails when the HTTP client cannot be created.
    pub fn new(api_key: impl Into<String>) -> Result<Self, WebError> {
        Ok(Self {
            client: api_client()?,
            api_key: api_key.into(),
            endpoint: TAVILY_ENDPOINT.into(),
        })
    }

    /// Point at another endpoint (tests, proxies).
    #[must_use]
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }
}

#[async_trait]
impl SearchProvider for TavilySearch {
    fn name(&self) -> &'static str {
        "Tavily"
    }

    async fn search(&self, query: &str, count: usize) -> Result<Vec<SearchHit>, WebError> {
        let request = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({
                "query": query,
                "max_results": count,
                "search_depth": "basic",
            }));
        let body: ResultsResponse = send_json(self.name(), &self.endpoint, request).await?;
        Ok(body
            .results
            .into_iter()
            .map(|r| hit(&r.title, r.url, &r.content))
            .collect())
    }
}

/// A SearXNG instance's JSON API (`format=json` must be enabled in its
/// `settings.yml`).
pub struct SearxngSearch {
    client: reqwest::Client,
    search_url: String,
}

impl SearxngSearch {
    /// `base_url` is the instance root, e.g. `http://localhost:8888`.
    ///
    /// # Errors
    /// Fails for a non-http(s) URL or when the HTTP client cannot be created.
    pub fn new(base_url: &str) -> Result<Self, WebError> {
        // The operator chose this instance, so local addresses are fine.
        let base = validate_url(base_url, true)?;
        let search_url = format!("{}/search", base.as_str().trim_end_matches('/'));
        Ok(Self {
            client: api_client()?,
            search_url,
        })
    }
}

#[async_trait]
impl SearchProvider for SearxngSearch {
    fn name(&self) -> &'static str {
        "SearXNG"
    }

    async fn search(&self, query: &str, count: usize) -> Result<Vec<SearchHit>, WebError> {
        let request = self
            .client
            .get(&self.search_url)
            .query(&[("q", query), ("format", "json")])
            .header(reqwest::header::ACCEPT, "application/json");
        let body: ResultsResponse = send_json(self.name(), &self.search_url, request)
            .await
            .map_err(|e| match e {
                WebError::Status { url, status, .. } if status == 403 => WebError::Status {
                    url,
                    status,
                    detail: " (enable `json` under `search.formats` in the instance's \
                             settings.yml)"
                        .into(),
                },
                other => other,
            })?;
        Ok(body
            .results
            .into_iter()
            .take(count)
            .map(|r| hit(&r.title, r.url, &r.content))
            .collect())
    }
}

#[derive(Deserialize)]
struct ResultsResponse {
    #[serde(default)]
    results: Vec<ContentResult>,
}

#[derive(Deserialize)]
struct ContentResult {
    #[serde(default)]
    title: String,
    url: String,
    #[serde(default)]
    content: String,
}

fn hit(title: &str, url: String, snippet: &str) -> SearchHit {
    SearchHit {
        title: html_to_text(title),
        url,
        snippet: html_to_text(snippet),
    }
}

async fn send_json<T: serde::de::DeserializeOwned>(
    provider: &'static str,
    endpoint: &str,
    request: reqwest::RequestBuilder,
) -> Result<T, WebError> {
    let response = request
        .send()
        .await
        .map_err(|e| WebError::from_reqwest(endpoint, &e))?;
    let status = response.status();
    let (body, _) = read_capped(endpoint, response, MAX_RESPONSE_BYTES).await?;
    if !status.is_success() {
        let text = String::from_utf8_lossy(&body);
        let excerpt = super::super::truncate_chars(text.trim(), 300);
        let hint = match status.as_u16() {
            401 | 403 => " (check the API key)",
            429 => " (rate limited; try again later)",
            _ => "",
        };
        return Err(WebError::Status {
            url: endpoint.into(),
            status: status.as_u16(),
            detail: if excerpt.is_empty() {
                hint.to_string()
            } else {
                format!("{hint}: {excerpt}")
            },
        });
    }
    serde_json::from_slice(&body).map_err(|e| WebError::BadResponse {
        provider,
        reason: e.to_string(),
    })
}
