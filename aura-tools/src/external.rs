//! External tool support via HTTP POST callbacks.
//!
//! External tools implement the [`Tool`] trait and dispatch execution
//! to a remote service via HTTP POST to a callback URL.

use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core::{ExternalToolDefinition, ToolResult};
use aura_reasoner::ToolDefinition;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

/// Request body POSTed to the external tool's callback URL.
#[derive(Debug, Serialize)]
struct ExternalToolRequest {
    tool_name: String,
    input: serde_json::Value,
}

/// Expected response body from the external tool callback.
#[derive(Debug, Deserialize)]
struct ExternalToolResponse {
    /// Tool output (text).
    #[serde(default)]
    output: String,
    /// Whether the tool execution succeeded.
    #[serde(default = "default_true")]
    ok: bool,
    /// Optional error message.
    #[serde(default)]
    error: Option<String>,
}

const fn default_true() -> bool {
    true
}

/// An external tool that dispatches execution via HTTP POST.
pub struct ExternalTool {
    def: ExternalToolDefinition,
    client: reqwest::Client,
}

impl ExternalTool {
    /// Create a new external tool from a definition.
    #[must_use]
    pub fn new(def: ExternalToolDefinition) -> Self {
        Self {
            def,
            client: reqwest::Client::new(),
        }
    }

    /// Create a new external tool with a custom HTTP client.
    #[must_use]
    pub fn with_client(def: ExternalToolDefinition, client: reqwest::Client) -> Self {
        Self { def, client }
    }
}

#[async_trait]
impl Tool for ExternalTool {
    fn name(&self) -> &str {
        &self.def.name
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.def.name.clone(),
            description: self.def.description.clone(),
            input_schema: self.def.input_schema.clone(),
        }
    }

    #[instrument(skip(self, _ctx, args), fields(tool = %self.def.name, url = %self.def.callback_url))]
    async fn execute(
        &self,
        _ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        debug!("Dispatching to external tool callback");

        let request_body = ExternalToolRequest {
            tool_name: self.def.name.clone(),
            input: args,
        };

        let response = self
            .client
            .post(&self.def.callback_url)
            .json(&request_body)
            .send()
            .await
            .map_err(|e| {
                ToolError::ExternalToolError(format!(
                    "HTTP request to {} failed: {e}",
                    self.def.callback_url
                ))
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ToolError::ExternalToolError(format!(
                "External tool returned HTTP {status}: {body}"
            )));
        }

        let tool_response: ExternalToolResponse = response.json().await.map_err(|e| {
            ToolError::ExternalToolError(format!("Failed to parse callback response: {e}"))
        })?;

        if tool_response.ok {
            Ok(ToolResult::success(&self.def.name, tool_response.output))
        } else {
            let error_msg = tool_response
                .error
                .unwrap_or_else(|| "external tool failed".to_string());
            Ok(ToolResult::failure(&self.def.name, error_msg))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_external_tool_definition_roundtrip() {
        let def = ExternalToolDefinition {
            name: "my_tool".into(),
            description: "Does things".into(),
            input_schema: serde_json::json!({"type": "object"}),
            callback_url: "http://localhost:8080/tool".into(),
        };

        let json = serde_json::to_string(&def).unwrap();
        let parsed: ExternalToolDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "my_tool");
        assert_eq!(parsed.callback_url, "http://localhost:8080/tool");
    }

    #[test]
    fn test_external_tool_produces_correct_definition() {
        let def = ExternalToolDefinition {
            name: "ext_search".into(),
            description: "Search external index".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            }),
            callback_url: "http://example.com/search".into(),
        };

        let tool = ExternalTool::new(def);
        assert_eq!(tool.name(), "ext_search");

        let tool_def = tool.definition();
        assert_eq!(tool_def.name, "ext_search");
        assert_eq!(tool_def.description, "Search external index");
        assert!(tool_def.input_schema["properties"]["query"].is_object());
    }
}
