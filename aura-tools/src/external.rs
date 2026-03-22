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

/// Request body posted to the external tool's callback URL.
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

const DEFAULT_CALLBACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

impl ExternalTool {
    /// Create a new external tool from a definition.
    ///
    /// # Errors
    /// Returns `ToolError::ExternalToolError` if the HTTP client cannot be built.
    pub(crate) fn new(def: ExternalToolDefinition) -> Result<Self, ToolError> {
        let client = reqwest::Client::builder()
            .timeout(DEFAULT_CALLBACK_TIMEOUT)
            .build()
            .map_err(|e| ToolError::ExternalToolError(format!("Failed to build HTTP client: {e}")))?;
        Ok(Self { def, client })
    }

    /// Create a new external tool with a custom HTTP client.
    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn with_client(def: ExternalToolDefinition, client: reqwest::Client) -> Self {
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
            cache_control: None,
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

        let tool = ExternalTool::new(def).unwrap();
        assert_eq!(tool.name(), "ext_search");

        let tool_def = tool.definition();
        assert_eq!(tool_def.name, "ext_search");
        assert_eq!(tool_def.description, "Search external index");
        assert!(tool_def.input_schema["properties"]["query"].is_object());
    }

    #[test]
    fn test_external_tool_with_client_constructor() {
        let def = ExternalToolDefinition {
            name: "custom_client_tool".into(),
            description: "Tool with custom client".into(),
            input_schema: serde_json::json!({"type": "object"}),
            callback_url: "http://localhost:9999/callback".into(),
        };

        let client = reqwest::Client::new();
        let tool = ExternalTool::with_client(def, client);
        assert_eq!(tool.name(), "custom_client_tool");
    }

    #[test]
    fn test_external_tool_definition_cache_control_is_none() {
        let def = ExternalToolDefinition {
            name: "no_cache".into(),
            description: "No cache".into(),
            input_schema: serde_json::json!({}),
            callback_url: "http://localhost/tool".into(),
        };

        let tool = ExternalTool::new(def).unwrap();
        assert!(tool.definition().cache_control.is_none());
    }

    #[test]
    fn test_external_tool_response_deserialization_defaults() {
        let json = r#"{"output": "hello"}"#;
        let resp: ExternalToolResponse = serde_json::from_str(json).unwrap();
        assert!(resp.ok); // default_true
        assert!(resp.error.is_none());
        assert_eq!(resp.output, "hello");
    }

    #[test]
    fn test_external_tool_response_deserialization_failure() {
        let json = r#"{"ok": false, "error": "bad input"}"#;
        let resp: ExternalToolResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.ok);
        assert_eq!(resp.error.as_deref(), Some("bad input"));
        assert!(resp.output.is_empty());
    }

    #[test]
    fn test_external_tool_response_deserialization_empty() {
        let json = r#"{}"#;
        let resp: ExternalToolResponse = serde_json::from_str(json).unwrap();
        assert!(resp.ok);
        assert!(resp.output.is_empty());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_external_tool_request_serialization() {
        let req = ExternalToolRequest {
            tool_name: "my_tool".into(),
            input: serde_json::json!({"key": "value"}),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["tool_name"], "my_tool");
        assert_eq!(json["input"]["key"], "value");
    }
}
