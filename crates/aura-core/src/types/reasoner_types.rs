//! Types shared with the reasoner layer: tool definitions, cache control, and tool result content.

use serde::{Deserialize, Serialize};

// ============================================================================
// Cache Control
// ============================================================================

/// Prompt-caching directive attached to tool definitions or content blocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheControl {
    /// Cache type (e.g., `"ephemeral"`).
    #[serde(rename = "type")]
    pub cache_type: String,
}

impl CacheControl {
    /// Create an ephemeral cache control directive.
    #[must_use]
    pub fn ephemeral() -> Self {
        Self {
            cache_type: "ephemeral".to_string(),
        }
    }
}

// ============================================================================
// Tool Definition
// ============================================================================

/// Tool definition for the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool name (e.g., "fs.read", "search.code")
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// JSON Schema for input parameters
    pub input_schema: serde_json::Value,
    /// Optional prompt-caching directive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

impl ToolDefinition {
    /// Create a new tool definition.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            cache_control: None,
        }
    }
}

// ============================================================================
// Tool Result Content
// ============================================================================

/// Content of a tool result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolResultContent {
    /// Plain text result
    Text(String),
    /// Structured JSON result
    Json(serde_json::Value),
}

impl ToolResultContent {
    /// Create text content.
    #[must_use]
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text(s.into())
    }

    /// Create JSON content.
    #[must_use]
    pub const fn json(value: serde_json::Value) -> Self {
        Self::Json(value)
    }
}

impl From<String> for ToolResultContent {
    fn from(s: String) -> Self {
        Self::Text(s)
    }
}

impl From<&str> for ToolResultContent {
    fn from(s: &str) -> Self {
        Self::Text(s.to_string())
    }
}

impl From<serde_json::Value> for ToolResultContent {
    fn from(v: serde_json::Value) -> Self {
        Self::Json(v)
    }
}
