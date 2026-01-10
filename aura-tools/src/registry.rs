//! Tool registry for managing available tools.
//!
//! Provides tool definitions and schemas for the model to use.

use aura_reasoner::ToolDefinition;
use std::collections::HashMap;

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
pub struct DefaultToolRegistry {
    tools: HashMap<String, ToolDefinition>,
}

impl DefaultToolRegistry {
    /// Create a new registry with all default tools.
    #[must_use]
    pub fn new() -> Self {
        let mut tools = HashMap::new();

        // Filesystem tools
        tools.insert("fs_ls".into(), fs_ls_schema());
        tools.insert("fs_read".into(), fs_read_schema());
        tools.insert("fs_stat".into(), fs_stat_schema());
        tools.insert("fs_write".into(), fs_write_schema());
        tools.insert("fs_edit".into(), fs_edit_schema());

        // Search tools
        tools.insert("search_code".into(), search_code_schema());

        // Command tools
        tools.insert("cmd_run".into(), cmd_run_schema());

        Self { tools }
    }

    /// Create a registry with only read-only tools.
    #[must_use]
    pub fn read_only() -> Self {
        let mut tools = HashMap::new();

        // Only safe read-only tools
        tools.insert("fs_ls".into(), fs_ls_schema());
        tools.insert("fs_read".into(), fs_read_schema());
        tools.insert("fs_stat".into(), fs_stat_schema());
        tools.insert("search_code".into(), search_code_schema());

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
    pub fn register(&mut self, tool: ToolDefinition) {
        self.tools.insert(tool.name.clone(), tool);
    }

    /// Remove a tool.
    pub fn unregister(&mut self, name: &str) -> Option<ToolDefinition> {
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

    fn get(&self, name: &str) -> Option<ToolDefinition> {
        self.tools.get(name).cloned()
    }
}

// ============================================================================
// Tool Schemas
// ============================================================================

/// Schema for `fs_ls` tool.
fn fs_ls_schema() -> ToolDefinition {
    ToolDefinition {
        name: "fs_ls".into(),
        description:
            "List directory contents. Returns files and directories with their types and sizes."
                .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the directory to list (relative to workspace root)"
                }
            },
            "required": ["path"]
        }),
    }
}

/// Schema for `fs_read` tool.
fn fs_read_schema() -> ToolDefinition {
    ToolDefinition {
        name: "fs_read".into(),
        description: "Read the contents of a file. Use this to examine source code, configuration files, and other text files.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read (relative to workspace root)"
                },
                "max_bytes": {
                    "type": "integer",
                    "description": "Maximum bytes to read (default: 1MB). Useful for large files."
                }
            },
            "required": ["path"]
        }),
    }
}

/// Schema for `fs_stat` tool.
fn fs_stat_schema() -> ToolDefinition {
    ToolDefinition {
        name: "fs_stat".into(),
        description: "Get file or directory metadata including size, type, and permissions.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file or directory (relative to workspace root)"
                }
            },
            "required": ["path"]
        }),
    }
}

/// Schema for `fs_write` tool.
fn fs_write_schema() -> ToolDefinition {
    ToolDefinition {
        name: "fs_write".into(),
        description:
            "Write content to a file. Creates the file if it doesn't exist, overwrites if it does."
                .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to write (relative to workspace root)"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                },
                "create_dirs": {
                    "type": "boolean",
                    "description": "Create parent directories if they don't exist (default: false)"
                }
            },
            "required": ["path", "content"]
        }),
    }
}

/// Schema for `fs_edit` tool.
fn fs_edit_schema() -> ToolDefinition {
    ToolDefinition {
        name: "fs_edit".into(),
        description: "Edit an existing file by replacing a specific portion of text. Use this for targeted modifications.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit (relative to workspace root)"
                },
                "old_text": {
                    "type": "string",
                    "description": "The exact text to find and replace"
                },
                "new_text": {
                    "type": "string",
                    "description": "The text to replace it with"
                }
            },
            "required": ["path", "old_text", "new_text"]
        }),
    }
}

/// Schema for `search_code` tool.
fn search_code_schema() -> ToolDefinition {
    ToolDefinition {
        name: "search_code".into(),
        description: "Search for patterns in code using regex. Useful for finding function definitions, usages, and patterns across files.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Search pattern (regex supported)"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (default: workspace root)"
                },
                "file_pattern": {
                    "type": "string",
                    "description": "Glob pattern for files to search (e.g., '*.rs', '*.ts')"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 100)"
                }
            },
            "required": ["pattern"]
        }),
    }
}

/// Schema for `cmd_run` tool.
fn cmd_run_schema() -> ToolDefinition {
    ToolDefinition {
        name: "cmd_run".into(),
        description: "Run a shell command. Use with caution. Only allowed commands will execute."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "program": {
                    "type": "string",
                    "description": "The program/command to run"
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Command arguments"
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory (default: workspace root)"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default: 30000)"
                }
            },
            "required": ["program"]
        }),
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
        assert!(registry.has("fs_read"));
        assert!(registry.has("fs_write"));
        assert!(registry.has("search_code"));
        assert!(registry.has("cmd_run"));
    }

    #[test]
    fn test_read_only_registry() {
        let registry = DefaultToolRegistry::read_only();
        let _tools = registry.list();

        assert!(registry.has("fs_read"));
        assert!(registry.has("fs_ls"));
        assert!(registry.has("search_code"));
        assert!(!registry.has("fs_write"));
        assert!(!registry.has("cmd_run"));
    }

    #[test]
    fn test_get_tool() {
        let registry = DefaultToolRegistry::new();
        let tool = registry.get("fs_read").unwrap();

        assert_eq!(tool.name, "fs_read");
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
        assert!(registry.has("cmd_run"));

        registry.unregister("cmd_run");
        assert!(!registry.has("cmd_run"));
    }

    #[test]
    fn test_tool_schema_validity() {
        let registry = DefaultToolRegistry::new();

        for tool in registry.list() {
            // Each tool should have valid JSON Schema structure
            assert!(tool.input_schema.is_object());
            let schema = tool.input_schema.as_object().unwrap();
            assert!(schema.contains_key("type"));
            assert!(schema.contains_key("properties"));
        }
    }
}
