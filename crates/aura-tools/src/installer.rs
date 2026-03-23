//! Thread-safe tool installer for managing installed tool definitions.
//!
//! The `ToolInstaller` serves as the harness-level registry for tools
//! loaded from `tools.toml` or installed via the HTTP API.

use crate::config::{load_tools_from_file, ToolConfigError};
use aura_core::InstalledToolDefinition;
use aura_reasoner::ToolDefinition;
use std::collections::HashMap;
use std::path::Path;
use std::sync::RwLock;
use tracing::{debug, info};

/// Thread-safe registry of installed tool definitions.
///
/// Shared as `Arc<ToolInstaller>` across sessions and the HTTP API.
pub struct ToolInstaller {
    tools: RwLock<HashMap<String, InstalledToolDefinition>>,
}

impl ToolInstaller {
    /// Create a new empty installer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tools: RwLock::new(HashMap::new()),
        }
    }

    /// Install (or replace) a tool definition.
    pub fn install(&self, def: InstalledToolDefinition) {
        info!(tool = %def.name, "Installing tool");
        self.tools
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(def.name.clone(), def);
    }

    /// Uninstall a tool by name. Returns `true` if the tool existed.
    pub fn uninstall(&self, name: &str) -> bool {
        info!(tool = %name, "Uninstalling tool");
        self.tools
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(name)
            .is_some()
    }

    /// Load tools from a TOML config file.
    ///
    /// Returns the number of tools loaded.
    ///
    /// # Errors
    /// Returns `ToolConfigError` if the file cannot be read or parsed.
    pub fn load_from_file(&self, path: &Path) -> Result<usize, ToolConfigError> {
        let defs = load_tools_from_file(path)?;
        let count = defs.len();
        let mut tools = self
            .tools
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for def in defs {
            debug!(tool = %def.name, "Loading tool from config");
            tools.insert(def.name.clone(), def);
        }
        info!(count, path = %path.display(), "Loaded tools from config file");
        Ok(count)
    }

    /// Take a snapshot of all installed tool definitions.
    #[must_use]
    pub fn snapshot(&self) -> Vec<InstalledToolDefinition> {
        self.tools
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }

    /// Get model-facing `ToolDefinition`s for all installed tools.
    #[must_use]
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .map(|def| ToolDefinition {
                name: def.name.clone(),
                description: def.description.clone(),
                input_schema: def.input_schema.clone(),
                cache_control: None,
            })
            .collect()
    }

    /// Get the names of all installed tools.
    #[must_use]
    pub fn tool_names(&self) -> Vec<String> {
        self.tools
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }

    /// Get the number of installed tools.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Check if the installer is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for ToolInstaller {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ToolInstaller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.len();
        f.debug_struct("ToolInstaller")
            .field("tool_count", &count)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core::ToolAuth;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn sample_def(name: &str) -> InstalledToolDefinition {
        InstalledToolDefinition {
            name: name.into(),
            description: format!("Tool {name}"),
            input_schema: serde_json::json!({"type": "object"}),
            endpoint: format!("http://localhost:8080/{name}"),
            auth: ToolAuth::None,
            timeout_ms: None,
            namespace: None,
            metadata: Default::default(),
        }
    }

    #[test]
    fn test_install_and_snapshot() {
        let installer = ToolInstaller::new();
        assert!(installer.is_empty());

        installer.install(sample_def("tool_a"));
        installer.install(sample_def("tool_b"));

        assert_eq!(installer.len(), 2);
        let snap = installer.snapshot();
        assert_eq!(snap.len(), 2);
    }

    #[test]
    fn test_install_replaces_existing() {
        let installer = ToolInstaller::new();
        installer.install(sample_def("tool_a"));

        let mut updated = sample_def("tool_a");
        updated.description = "Updated description".into();
        installer.install(updated);

        assert_eq!(installer.len(), 1);
        let snap = installer.snapshot();
        assert_eq!(snap[0].description, "Updated description");
    }

    #[test]
    fn test_uninstall() {
        let installer = ToolInstaller::new();
        installer.install(sample_def("tool_a"));
        installer.install(sample_def("tool_b"));

        assert!(installer.uninstall("tool_a"));
        assert_eq!(installer.len(), 1);

        assert!(!installer.uninstall("nonexistent"));
    }

    #[test]
    fn test_tool_names() {
        let installer = ToolInstaller::new();
        installer.install(sample_def("alpha"));
        installer.install(sample_def("beta"));

        let mut names = installer.tool_names();
        names.sort();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn test_definitions() {
        let installer = ToolInstaller::new();
        installer.install(sample_def("tool_a"));

        let defs = installer.definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "tool_a");
        assert!(defs[0].cache_control.is_none());
    }

    #[test]
    fn test_load_from_file() {
        let toml_content = r#"
[[tool]]
name = "loaded_tool"
description = "A loaded tool"
endpoint = "http://localhost:8080/loaded"

[tool.input_schema]
type = "object"
"#;

        let mut file = NamedTempFile::new().unwrap();
        file.write_all(toml_content.as_bytes()).unwrap();

        let installer = ToolInstaller::new();
        let count = installer.load_from_file(file.path()).unwrap();
        assert_eq!(count, 1);
        assert_eq!(installer.len(), 1);

        let names = installer.tool_names();
        assert_eq!(names, vec!["loaded_tool"]);
    }

    #[test]
    fn test_default_impl() {
        let installer = ToolInstaller::default();
        assert!(installer.is_empty());
    }

    #[test]
    fn test_debug_impl() {
        let installer = ToolInstaller::new();
        installer.install(sample_def("tool_a"));
        let debug = format!("{installer:?}");
        assert!(debug.contains("tool_count: 1"));
    }

    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;
        let installer = Arc::new(ToolInstaller::new());

        let handles: Vec<_> = (0..10)
            .map(|i| {
                let inst = Arc::clone(&installer);
                std::thread::spawn(move || {
                    inst.install(sample_def(&format!("tool_{i}")));
                    assert!(inst.len() > 0);
                    let _ = inst.snapshot();
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(installer.len(), 10);
    }
}
