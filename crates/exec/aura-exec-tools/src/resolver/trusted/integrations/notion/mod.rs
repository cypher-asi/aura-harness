//! Notion trusted-integration handlers.
//!
//! The handlers and the HTTP calls live here; request bodies are built in
//! [`payloads`], responses shaped in [`responses`] and arguments read in
//! [`args`].

mod args;
mod payloads;
mod responses;

use super::super::super::json_paths::{optional_string, required_string};
use super::super::ToolResolver;
use crate::error::ToolError;
use args::{
    insert_optional_page_size, insert_optional_string, notion_content_mode, notion_page_size,
    optional_bool, required_notion_id,
};
use aura_core_types::{InstalledToolRuntimeIntegration, InstalledToolRuntimeProviderExecution};
use payloads::{
    append_blocks_payload, create_data_source_payload, create_database_payload,
    create_page_payload, query_data_source_payload, update_page_markdown_payload,
    update_page_payload,
};
use reqwest::{Method, Url};
use responses::{
    notion_block_list_response, notion_data_source_summary, notion_database_summary,
    notion_page_list_response, notion_page_summary,
};
use serde_json::{json, Map, Value};

impl ToolResolver {
    pub(in super::super) async fn notion_search_pages(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        args: &Value,
    ) -> Result<Value, ToolError> {
        let query = required_string(args, &["query"])?;
        let mut payload = Map::new();
        payload.insert("query".to_string(), Value::String(query));
        payload.insert(
            "filter".to_string(),
            json!({ "property": "object", "value": "page" }),
        );
        insert_optional_page_size(&mut payload, args);
        insert_optional_string(
            &mut payload,
            args,
            &["start_cursor", "startCursor"],
            "start_cursor",
        );

        let response = self
            .notion_json_request(
                Method::POST,
                provider,
                integration,
                "/search",
                None,
                Some(Value::Object(payload)),
            )
            .await?;
        Ok(notion_page_list_response(&response))
    }

    pub(in super::super) async fn notion_fetch_page(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        args: &Value,
    ) -> Result<Value, ToolError> {
        let page_id = required_notion_id(args, &["page_id", "pageId", "page"])?;
        let page = self
            .notion_json_request(
                Method::GET,
                provider,
                integration,
                &format!("/pages/{page_id}"),
                None,
                None,
            )
            .await?;

        let mut response = Map::new();
        response.insert("page".to_string(), notion_page_summary(&page));
        match notion_content_mode(args).as_str() {
            "metadata" | "summary" | "none" => {}
            "markdown" | "full" => {
                response.insert(
                    "markdown".to_string(),
                    self.notion_fetch_page_markdown(provider, integration, &page_id, args)
                        .await?,
                );
            }
            "blocks" => {
                response.insert(
                    "blocks".to_string(),
                    self.notion_block_children_response(provider, integration, &page_id, args)
                        .await?,
                );
            }
            "both" => {
                response.insert(
                    "markdown".to_string(),
                    self.notion_fetch_page_markdown(provider, integration, &page_id, args)
                        .await?,
                );
                response.insert(
                    "blocks".to_string(),
                    self.notion_block_children_response(provider, integration, &page_id, args)
                        .await?,
                );
            }
            mode => {
                return Err(ToolError::ExternalToolError(format!(
                    "unsupported notion content_mode `{mode}`"
                )));
            }
        }
        Ok(Value::Object(response))
    }

    pub(in super::super) async fn notion_get_block_children(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        args: &Value,
    ) -> Result<Value, ToolError> {
        let block_id = required_notion_id(args, &["block_id", "blockId", "page_id", "pageId"])?;
        self.notion_block_children_response(provider, integration, &block_id, args)
            .await
    }

    pub(in super::super) async fn notion_append_block_children(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        args: &Value,
    ) -> Result<Value, ToolError> {
        let block_id = required_notion_id(args, &["block_id", "blockId", "page_id", "pageId"])?;
        let response = self
            .notion_json_request(
                Method::PATCH,
                provider,
                integration,
                &format!("/blocks/{block_id}/children"),
                None,
                Some(append_blocks_payload(args)?),
            )
            .await?;
        Ok(notion_block_list_response(&response))
    }

    pub(in super::super) async fn notion_create_page(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        args: &Value,
    ) -> Result<Value, ToolError> {
        let response = self
            .notion_json_request(
                Method::POST,
                provider,
                integration,
                "/pages",
                None,
                Some(create_page_payload(args)?),
            )
            .await?;
        Ok(json!({ "page": notion_page_summary(&response) }))
    }

    pub(in super::super) async fn notion_update_page(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        args: &Value,
    ) -> Result<Value, ToolError> {
        let page_id = required_notion_id(args, &["page_id", "pageId", "page"])?;
        let response = self
            .notion_json_request(
                Method::PATCH,
                provider,
                integration,
                &format!("/pages/{page_id}"),
                None,
                Some(update_page_payload(args)?),
            )
            .await?;
        Ok(json!({ "page": notion_page_summary(&response) }))
    }

    pub(in super::super) async fn notion_update_page_markdown(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        args: &Value,
    ) -> Result<Value, ToolError> {
        let page_id = required_notion_id(args, &["page_id", "pageId", "page"])?;
        self.notion_json_request(
            Method::PATCH,
            provider,
            integration,
            &format!("/pages/{page_id}/markdown"),
            None,
            Some(update_page_markdown_payload(args)?),
        )
        .await
    }

    pub(in super::super) async fn notion_query_data_source(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        args: &Value,
    ) -> Result<Value, ToolError> {
        let data_source_id = required_notion_id(args, &["data_source_id", "dataSourceId"])?;
        let response = self
            .notion_json_request(
                Method::POST,
                provider,
                integration,
                &format!("/data_sources/{data_source_id}/query"),
                None,
                Some(query_data_source_payload(args)?),
            )
            .await?;
        Ok(notion_page_list_response(&response))
    }

    pub(in super::super) async fn notion_create_database(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        args: &Value,
    ) -> Result<Value, ToolError> {
        let response = self
            .notion_json_request(
                Method::POST,
                provider,
                integration,
                "/databases",
                None,
                Some(create_database_payload(args)?),
            )
            .await?;
        Ok(json!({ "database": notion_database_summary(&response) }))
    }

    pub(in super::super) async fn notion_create_data_source(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        args: &Value,
    ) -> Result<Value, ToolError> {
        let response = self
            .notion_json_request(
                Method::POST,
                provider,
                integration,
                "/data_sources",
                None,
                Some(create_data_source_payload(args)?),
            )
            .await?;
        Ok(json!({ "data_source": notion_data_source_summary(&response) }))
    }

    async fn notion_fetch_page_markdown(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        page_id: &str,
        args: &Value,
    ) -> Result<Value, ToolError> {
        let query = optional_bool(args, &["include_transcript", "includeTranscript"])
            .unwrap_or(false)
            .then(|| vec![("include_transcript".to_string(), "true".to_string())]);
        self.notion_json_request(
            Method::GET,
            provider,
            integration,
            &format!("/pages/{page_id}/markdown"),
            query,
            None,
        )
        .await
    }

    async fn notion_block_children_response(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        block_id: &str,
        args: &Value,
    ) -> Result<Value, ToolError> {
        let mut query = Vec::new();
        if let Some(cursor) = optional_string(args, &["start_cursor", "startCursor"]) {
            query.push(("start_cursor".to_string(), cursor));
        }
        if let Some(page_size) = notion_page_size(args) {
            query.push(("page_size".to_string(), page_size.to_string()));
        }
        let response = self
            .notion_json_request(
                Method::GET,
                provider,
                integration,
                &format!("/blocks/{block_id}/children"),
                Some(query),
                None,
            )
            .await?;
        Ok(notion_block_list_response(&response))
    }

    async fn notion_json_request(
        &self,
        method: Method,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        path: &str,
        query: Option<Vec<(String, String)>>,
        body: Option<Value>,
    ) -> Result<Value, ToolError> {
        let mut url = Url::parse(&format!(
            "{}{}",
            notion_base_url(provider, integration),
            path
        ))
        .map_err(|e| ToolError::ExternalToolError(format!("invalid notion url: {e}")))?;
        if let Some(query) = query {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query {
                pairs.append_pair(&key, &value);
            }
        }
        self.provider_json_request(method, url.as_str(), provider, integration, body)
            .await
    }
}

fn notion_base_url(
    provider: &InstalledToolRuntimeProviderExecution,
    integration: &InstalledToolRuntimeIntegration,
) -> String {
    integration
        .base_url
        .as_deref()
        .unwrap_or(&provider.base_url)
        .trim_end_matches('/')
        .to_string()
}
