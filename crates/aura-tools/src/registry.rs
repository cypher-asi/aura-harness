//! Tool registry for managing available tools.
//!
//! Provides tool definitions and schemas for the model to use.

use crate::tool::{builtin_tools, read_only_builtin_tools};
use aura_reasoner::ToolDefinition;
use std::collections::HashMap;
use tracing::{debug, instrument};

// ============================================================================
// ToolRegistry Trait
// ============================================================================

/// Registry of available tools.
pub trait ToolRegistry: Send + Sync {
    /// List all available tools.
    fn list(&self) -> Vec<ToolDefinition>;

    /// Get a specific tool definition.
    fn get(&self, name: &str) -> Option<ToolDefinition>;

    /// Check if a tool exists.
    fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }
}

// ============================================================================
// DefaultToolRegistry
// ============================================================================

/// Default tool registry with built-in tools.
///
/// Populates definitions from [`Tool::definition()`](crate::tool::Tool::definition)
/// rather than maintaining separate schema functions.
pub struct DefaultToolRegistry {
    tools: HashMap<String, ToolDefinition>,
}

impl DefaultToolRegistry {
    /// Create a new registry with all default tools.
    #[must_use]
    #[instrument(skip_all)]
    pub fn new() -> Self {
        let mut tools = HashMap::new();
        for tool in builtin_tools() {
            let def = tool.definition();
            tools.insert(def.name.clone(), def);
        }
        debug!(
            tool_count = tools.len(),
            "Initialized default tool registry"
        );
        Self { tools }
    }

    /// Create a registry with only read-only tools.
    #[must_use]
    #[instrument(skip_all)]
    pub fn read_only() -> Self {
        let mut tools = HashMap::new();
        for tool in read_only_builtin_tools() {
            let def = tool.definition();
            tools.insert(def.name.clone(), def);
        }
        debug!(
            tool_count = tools.len(),
            "Initialized read-only tool registry"
        );
        Self { tools }
    }

    /// Create an empty registry (for testing).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Add a custom tool.
    #[instrument(skip(self, tool), fields(tool_name = %tool.name))]
    pub fn register(&mut self, tool: ToolDefinition) {
        debug!("Registering custom tool");
        self.tools.insert(tool.name.clone(), tool);
    }

    /// Remove a tool.
    #[instrument(skip(self), fields(tool_name = %name))]
    pub fn unregister(&mut self, name: &str) -> Option<ToolDefinition> {
        debug!("Unregistering tool");
        self.tools.remove(name)
    }
}

impl Default for DefaultToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry for DefaultToolRegistry {
    fn list(&self) -> Vec<ToolDefinition> {
        self.tools.values().cloned().collect()
    }

    #[instrument(skip(self), fields(tool_name = %name))]
    fn get(&self, name: &str) -> Option<ToolDefinition> {
        self.tools.get(name).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_registry() {
        let registry = DefaultToolRegistry::new();
        let tools = registry.list();

        assert!(tools.len() >= 7);
        assert!(registry.has("read_file"));
        assert!(registry.has("write_file"));
        assert!(registry.has("search_code"));
        assert!(registry.has("run_command"));
    }

    #[test]
    fn test_read_only_registry() {
        let registry = DefaultToolRegistry::read_only();
        let _tools = registry.list();

        assert!(registry.has("read_file"));
        assert!(registry.has("list_files"));
        assert!(registry.has("search_code"));
        assert!(!registry.has("write_file"));
        assert!(!registry.has("run_command"));
    }

    #[test]
    fn test_get_tool() {
        let registry = DefaultToolRegistry::new();
        let tool = registry.get("read_file").unwrap();

        assert_eq!(tool.name, "read_file");
        assert!(!tool.description.is_empty());
        assert!(tool.input_schema.get("properties").is_some());
    }

    #[test]
    fn test_custom_tool() {
        let mut registry = DefaultToolRegistry::empty();
        registry.register(ToolDefinition::new(
            "custom.tool",
            "A custom tool",
            serde_json::json!({ "type": "object" }),
        ));

        assert!(registry.has("custom.tool"));
        assert_eq!(registry.list().len(), 1);
    }

    #[test]
    fn test_unregister_tool() {
        let mut registry = DefaultToolRegistry::new();
        assert!(registry.has("run_command"));

        registry.unregister("run_command");
        assert!(!registry.has("run_command"));
    }

    #[test]
    fn test_tool_schema_validity() {
        let registry = DefaultToolRegistry::new();

        for tool in registry.list() {
            assert!(tool.input_schema.is_object());
            let schema = tool.input_schema.as_object().unwrap();
            assert!(schema.contains_key("type"));
            assert!(schema.contains_key("properties"));
        }
    }
}
