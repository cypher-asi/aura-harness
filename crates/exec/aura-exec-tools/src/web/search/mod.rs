//! `web_search`: query a configured search provider.
//!
//! The provider is chosen once at startup from the environment (see
//! [`WebSearchSettings::from_lookup`]). With none configured the tool still
//! exists and answers every call with what to set, so the user learns how
//! to turn it on instead of the model silently lacking it.

mod providers;

pub use providers::{BraveSearch, SearxngSearch, TavilySearch};

use super::{WebError, REQUEST_TIMEOUT};
use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core_types::{ToolDefinition, ToolResult};
use serde::Deserialize;
use std::fmt;
use std::sync::Arc;

/// Tool name used for dispatch.
pub const WEB_SEARCH_TOOL_NAME: &str = "web_search";

const DEFAULT_RESULTS: usize = 8;
const MAX_RESULTS: usize = 20;

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// A web search backend.
#[async_trait]
pub trait SearchProvider: Send + Sync {
    /// Human-readable name shown with results.
    fn name(&self) -> &'static str;

    /// Up to `count` results for `query`.
    async fn search(&self, query: &str, count: usize) -> Result<Vec<SearchHit>, WebError>;
}

/// Which provider to build, and with what.
#[derive(Clone, PartialEq, Eq)]
pub enum ProviderChoice {
    Brave { api_key: String },
    Tavily { api_key: String },
    Searxng { base_url: String },
}

impl fmt::Debug for ProviderChoice {
    // Keys stay out of logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Brave { .. } => write!(f, "Brave"),
            Self::Tavily { .. } => write!(f, "Tavily"),
            Self::Searxng { base_url } => write!(f, "Searxng({base_url})"),
        }
    }
}

/// How `web_search` is configured, read once at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSearchSettings {
    /// The provider, or why there is none (shown to the model and user).
    pub provider: Result<ProviderChoice, String>,
}

const NOT_CONFIGURED: &str = "web_search is not configured. Set BRAVE_API_KEY (Brave Search), \
     TAVILY_API_KEY (Tavily) or AURA_SEARXNG_URL (a SearXNG instance with JSON output enabled), \
     then restart aura. Use web_fetch if you already know the URL.";

impl WebSearchSettings {
    /// Pick a provider from the variables visible through `get`:
    ///
    /// - `AURA_WEB_SEARCH_PROVIDER` (`brave`, `tavily`, `searxng` or `off`)
    ///   forces a choice; its key must then be set.
    /// - Otherwise the first configured of `BRAVE_API_KEY` (or
    ///   `BRAVE_SEARCH_API_KEY`), `TAVILY_API_KEY` and `AURA_SEARXNG_URL`.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let value = |name: &str| {
            get(name)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let brave = || value("BRAVE_API_KEY").or_else(|| value("BRAVE_SEARCH_API_KEY"));
        let tavily = || value("TAVILY_API_KEY");
        let searxng = || value("AURA_SEARXNG_URL");
        let missing = |provider: &str, var: &str| {
            format!(
                "web_search is not configured: AURA_WEB_SEARCH_PROVIDER={provider} but {var} \
                 is not set."
            )
        };

        let provider = match value("AURA_WEB_SEARCH_PROVIDER").map(|p| p.to_ascii_lowercase()) {
            Some(p) if p == "brave" => brave()
                .map(|api_key| ProviderChoice::Brave { api_key })
                .ok_or_else(|| missing("brave", "BRAVE_API_KEY")),
            Some(p) if p == "tavily" => tavily()
                .map(|api_key| ProviderChoice::Tavily { api_key })
                .ok_or_else(|| missing("tavily", "TAVILY_API_KEY")),
            Some(p) if p == "searxng" => searxng()
                .map(|base_url| ProviderChoice::Searxng { base_url })
                .ok_or_else(|| missing("searxng", "AURA_SEARXNG_URL")),
            Some(p) if p == "off" || p == "none" => {
                Err("web_search is turned off (AURA_WEB_SEARCH_PROVIDER=off).".into())
            }
            Some(other) => Err(format!(
                "web_search is not configured: unknown AURA_WEB_SEARCH_PROVIDER `{other}` \
                 (use brave, tavily or searxng)."
            )),
            None => brave()
                .map(|api_key| ProviderChoice::Brave { api_key })
                .or_else(|| tavily().map(|api_key| ProviderChoice::Tavily { api_key }))
                .or_else(|| searxng().map(|base_url| ProviderChoice::Searxng { base_url }))
                .ok_or_else(|| NOT_CONFIGURED.to_string()),
        };
        Self { provider }
    }

    /// Provider name for logs and status lines, or `None`.
    #[must_use]
    pub fn provider_name(&self) -> Option<&'static str> {
        match self.provider.as_ref().ok()? {
            ProviderChoice::Brave { .. } => Some("Brave Search"),
            ProviderChoice::Tavily { .. } => Some("Tavily"),
            ProviderChoice::Searxng { .. } => Some("SearXNG"),
        }
    }
}

/// Build the provider `settings` names.
///
/// # Errors
/// Returns the reason when nothing is configured or the provider cannot
/// be created.
pub fn build_provider(settings: &WebSearchSettings) -> Result<Arc<dyn SearchProvider>, String> {
    let built: Result<Arc<dyn SearchProvider>, WebError> = match settings.provider.clone()? {
        ProviderChoice::Brave { api_key } => {
            BraveSearch::new(api_key).map(|p| Arc::new(p) as Arc<dyn SearchProvider>)
        }
        ProviderChoice::Tavily { api_key } => {
            TavilySearch::new(api_key).map(|p| Arc::new(p) as Arc<dyn SearchProvider>)
        }
        ProviderChoice::Searxng { base_url } => {
            SearxngSearch::new(&base_url).map(|p| Arc::new(p) as Arc<dyn SearchProvider>)
        }
    };
    built.map_err(|e| format!("web_search is not available: {e}"))
}

/// The `web_search` tool.
#[derive(Clone)]
pub struct WebSearchTool {
    provider: Result<Arc<dyn SearchProvider>, String>,
}

impl WebSearchTool {
    /// Tool for the provider `settings` selects; an unconfigured tool
    /// answers every call with how to configure it.
    #[must_use]
    pub fn new(settings: &WebSearchSettings) -> Self {
        Self {
            provider: build_provider(settings),
        }
    }

    /// Tool backed by `provider`.
    #[must_use]
    pub fn with_provider(provider: Arc<dyn SearchProvider>) -> Self {
        Self {
            provider: Ok(provider),
        }
    }
}

#[derive(Deserialize)]
struct SearchArgs {
    query: String,
    #[serde(default)]
    count: Option<usize>,
}

fn render_hits(query: &str, provider: &str, hits: &[SearchHit]) -> String {
    if hits.is_empty() {
        return format!("No results for \"{query}\" ({provider}).");
    }
    let mut out = format!("Results for \"{query}\" ({provider}):\n");
    for (i, hit) in hits.iter().enumerate() {
        let title = if hit.title.is_empty() {
            hit.url.as_str()
        } else {
            hit.title.as_str()
        };
        out.push_str(&format!("\n{}. {title}\n   {}\n", i + 1, hit.url));
        if !hit.snippet.is_empty() {
            out.push_str(&format!("   {}\n", hit.snippet));
        }
    }
    out.push_str("\nUse web_fetch on a URL to read the page.");
    out
}

/// Definition sent to the model.
#[must_use]
pub fn web_search_definition() -> ToolDefinition {
    ToolDefinition {
        name: WEB_SEARCH_TOOL_NAME.into(),
        description: format!(
            "Search the web and get a numbered list of results (title, URL, snippet). Use it \
             for documentation, error messages, release notes and anything newer than your \
             training data, then read promising pages with web_fetch. Requests time out after \
             {}s.",
            REQUEST_TIMEOUT.as_secs()
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "What to search for."},
                "count": {
                    "type": "integer",
                    "description": format!("Number of results (default {DEFAULT_RESULTS}, max {MAX_RESULTS}).")
                }
            },
            "required": ["query"]
        }),
        cache_control: None,
        eager_input_streaming: None,
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        WEB_SEARCH_TOOL_NAME
    }

    fn definition(&self) -> ToolDefinition {
        web_search_definition()
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let args: SearchArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("web_search: {e}")))?;
        let query = args.query.trim();
        if query.is_empty() {
            return Err(ToolError::InvalidArguments(
                "web_search: query must not be empty".into(),
            ));
        }
        let provider = match &self.provider {
            Ok(provider) => provider,
            Err(why) => return Ok(ToolResult::failure(WEB_SEARCH_TOOL_NAME, why.clone())),
        };
        let count = args.count.unwrap_or(DEFAULT_RESULTS).clamp(1, MAX_RESULTS);
        match provider.search(query, count).await {
            Ok(hits) => Ok(ToolResult::success(
                WEB_SEARCH_TOOL_NAME,
                render_hits(query, provider.name(), &hits),
            )),
            Err(e) => Ok(ToolResult::failure(WEB_SEARCH_TOOL_NAME, e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests;
