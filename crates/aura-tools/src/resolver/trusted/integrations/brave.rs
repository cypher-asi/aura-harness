//! Brave Search trusted-integration handler — covers both the `web` and
//! `news` verticals with one impl, pulled in by both the tool-name
//! dispatch (`brave_search_web` / `brave_search_news`) and the spec
//! dispatch's `BraveSearch` arm. Response shaping is delegated to the
//! shared [`brave_search_results`] helper so the inline path and the
//! declarative `apply_result_transform::BraveSearch` produce
//! bit-identical envelopes.
//!
//! [`brave_search_results`]: super::super::transforms::brave_search_results

use super::super::super::json_paths::{optional_positive_number, optional_string, required_string};
use super::super::transforms::brave_search_results;
use super::super::ToolResolver;
use crate::error::ToolError;
use aura_core::{InstalledToolRuntimeIntegration, InstalledToolRuntimeProviderExecution};
use reqwest::{Method, Url};
use serde_json::Value;

impl ToolResolver {
    pub(in super::super) async fn brave_search(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        args: &Value,
        vertical: &str,
    ) -> Result<Value, ToolError> {
        let query = required_string(args, &["query", "q"])?;
        let mut url = Url::parse(&format!("{}/res/v1/{vertical}/search", provider.base_url))
            .map_err(|e| ToolError::ExternalToolError(format!("invalid brave base url: {e}")))?;
        {
            let mut params = url.query_pairs_mut();
            params.append_pair("q", &query);
            params.append_pair(
                "count",
                &optional_positive_number(args, &["count"])
                    .unwrap_or(10)
                    .to_string(),
            );
            if let Some(freshness) = optional_string(args, &["freshness"]) {
                params.append_pair("freshness", &freshness);
            }
            if let Some(country) = optional_string(args, &["country"]) {
                params.append_pair("country", &country);
            }
            if let Some(search_lang) = optional_string(args, &["search_lang", "searchLang"]) {
                params.append_pair("search_lang", &search_lang);
            }
        }
        let response = self
            .provider_json_request(Method::GET, url.as_str(), provider, integration, None)
            .await?;
        brave_search_results(&response, vertical, args)
    }
}
