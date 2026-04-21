//! Tool registry for managing available tools.
//!
//! Provides tool definitions and schemas for the model to use.

use crate::tool::{builtin_tools, read_only_builtin_tools};
use aura_core::{Registry, RegistryError, ToolDefinition};
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

/// Generic `Registry` impl (Wave 4 unification). The inherent
/// `register`/`unregister` methods are retained for ergonomic call
/// sites; this impl gives consumers the shared
/// [`aura_core::Registry`] abstraction.
impl Registry for DefaultToolRegistry {
    type Id = String;
    type Item = ToolDefinition;

    fn register(&mut self, id: Self::Id, item: Self::Item) -> Result<(), RegistryError> {
        if self.tools.contains_key(&id) {
            return Err(RegistryError::Duplicate(id));
        }
        self.tools.insert(id, item);
        Ok(())
    }

    fn get(&self, id: &Self::Id) -> Option<Self::Item> {
        self.tools.get(id).cloned()
    }

    fn iter(&self) -> Vec<(Self::Id, Self::Item)> {
        self.tools
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    fn remove(&mut self, id: &Self::Id) -> Option<Self::Item> {
        self.tools.remove(id)
    }

    fn len(&self) -> usize {
        self.tools.len()
    }

    fn is_empty(&self) -> bool {
        self.tools.is_empty()
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
        let tool = ToolRegistry::get(&registry, "read_file").unwrap();

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
    fn registry_trait_basic_ops() {
        use aura_core::Registry;

        let mut reg = DefaultToolRegistry::empty();
        assert!(Registry::is_empty(&reg));

        let def = ToolDefinition::new(
            "custom.tool",
            "A custom tool",
            serde_json::json!({ "type": "object" }),
        );
        Registry::register(&mut reg, "custom.tool".to_string(), def.clone())
            .expect("insert should succeed");
        assert_eq!(Registry::len(&reg), 1);

        let got = Registry::get(&reg, &"custom.tool".to_string()).expect("lookup");
        assert_eq!(got.name, "custom.tool");

        let err = Registry::register(&mut reg, "custom.tool".to_string(), def)
            .expect_err("duplicate must error");
        assert!(matches!(err, aura_core::RegistryError::Duplicate(ref id) if id == "custom.tool"));

        let snapshot: Vec<_> = Registry::iter(&reg).into_iter().map(|(k, _)| k).collect();
        assert_eq!(snapshot, vec!["custom.tool".to_string()]);

        let removed =
            Registry::remove(&mut reg, &"custom.tool".to_string()).expect("remove existing");
        assert_eq!(removed.name, "custom.tool");
        assert!(Registry::is_empty(&reg));
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
