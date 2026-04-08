//! Canonical tool catalog — single source of truth for tool metadata.
//!
//! Stores all tool entries (internal built-ins and schema-only definitions)
//! with profile and owner annotations.  Replaces the ad-hoc composition
//! previously spread across `DefaultToolRegistry` and the static lists in
//! `definitions.rs`.

use crate::definitions;
use crate::tool::builtin_tools;
use crate::ToolConfig;
use aura_core::{InstalledToolDefinition, ToolDefinition};
use std::collections::HashSet;
use tracing::debug;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Who provides execution for this tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolOwner {
    /// Executed by an internal handler (built-in `Tool` impl).
    Internal,
}

/// Runtime visibility profile.
///
/// `Core` ⊂ `Agent` and `Core` ⊂ `Engine` — querying for `Agent` or
/// `Engine` automatically includes all `Core` tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolProfile {
    /// Core tools only (fs, shell, search).
    Core,
    /// Chat agent: core + domain management tools.
    Agent,
    /// Task engine: core + engine-specific tools.
    Engine,
}

/// A single entry in the catalog.
#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub definition: ToolDefinition,
    pub owner: ToolOwner,
    /// Profiles that include this tool.
    pub profiles: Vec<ToolProfile>,
}

// ---------------------------------------------------------------------------
// ToolCatalog
// ---------------------------------------------------------------------------

/// Canonical catalog of every tool the system knows about.
///
/// Entries are populated at construction from `definitions.rs` and
/// `builtin_tools()`.
pub struct ToolCatalog {
    entries: Vec<CatalogEntry>,
}

impl ToolCatalog {
    /// Build the default catalog from all static tool definitions.
    #[must_use]
    pub fn new() -> Self {
        let mut entries = Vec::new();
        let mut seen = HashSet::new();

        let all_profiles = vec![ToolProfile::Core, ToolProfile::Agent, ToolProfile::Engine];

        // Core tools from curated definitions (model-facing schemas).
        for def in definitions::core_tool_definitions() {
            seen.insert(def.name.clone());
            entries.push(CatalogEntry {
                definition: def,
                owner: ToolOwner::Internal,
                profiles: all_profiles.clone(),
            });
        }

        // Built-in tool impls not covered by definitions.rs (e.g. stat_file).
        for tool in builtin_tools() {
            let name = tool.name().to_string();
            if seen.insert(name) {
                entries.push(CatalogEntry {
                    definition: tool.definition(),
                    owner: ToolOwner::Internal,
                    profiles: all_profiles.clone(),
                });
            }
        }

        // Agent-only management tools (spec, task, project, dev-loop).
        for def in definitions::chat_management_tools() {
            seen.insert(def.name.clone());
            entries.push(CatalogEntry {
                definition: def,
                owner: ToolOwner::Internal,
                profiles: vec![ToolProfile::Agent],
            });
        }

        // Engine-only tools (task_done, get_task_context, submit_plan).
        for def in definitions::engine_specific_tools() {
            seen.insert(def.name.clone());
            entries.push(CatalogEntry {
                definition: def,
                owner: ToolOwner::Internal,
                profiles: vec![ToolProfile::Engine],
            });
        }

        debug!(entry_count = entries.len(), "Built tool catalog");
        Self { entries }
    }

    // -----------------------------------------------------------------------
    // Visibility
    // -----------------------------------------------------------------------

    /// Get tool definitions for a profile **without** `ToolConfig` filtering.
    #[must_use]
    pub fn tools_for_profile(&self, profile: ToolProfile) -> Vec<ToolDefinition> {
        self.entries
            .iter()
            .filter(|e| e.profiles.contains(&profile))
            .map(|e| e.definition.clone())
            .collect()
    }

    /// Return a cloned catalog extended with installed tool definitions for the
    /// given profile. Existing tool names win to avoid shadowing built-ins.
    #[must_use]
    pub fn with_installed_tools(
        &self,
        profile: ToolProfile,
        installed_tools: &[InstalledToolDefinition],
    ) -> Self {
        if installed_tools.is_empty() {
            return Self {
                entries: self.entries.clone(),
            };
        }

        let mut entries = self.entries.clone();
        let mut seen = entries
            .iter()
            .map(|entry| entry.definition.name.clone())
            .collect::<HashSet<_>>();

        for tool in installed_tools {
            if !seen.insert(tool.name.clone()) {
                continue;
            }
            entries.push(CatalogEntry {
                definition: ToolDefinition::new(
                    tool.name.clone(),
                    tool.description.clone(),
                    tool.input_schema.clone(),
                ),
                owner: ToolOwner::Internal,
                profiles: vec![profile],
            });
        }

        Self { entries }
    }

    /// Get visible tools for a profile, filtered by `ToolConfig` permissions.
    #[must_use]
    pub fn visible_tools(&self, profile: ToolProfile, config: &ToolConfig) -> Vec<ToolDefinition> {
        let mut tools = self.tools_for_profile(profile);
        apply_config_filter(&mut tools, config);
        tools
    }

    /// Agent tools with a required `project_id` parameter (multi-project mode).
    #[must_use]
    pub fn visible_tools_multi_project(&self, config: &ToolConfig) -> Vec<ToolDefinition> {
        self.visible_tools(ToolProfile::Agent, config)
            .into_iter()
            .map(add_project_id_param)
            .collect()
    }

    /// Total static entry count.
    #[must_use]
    pub fn static_count(&self) -> usize {
        self.entries.len()
    }

    /// Determine the effective [`ToolOwner`] for a tool name.
    #[must_use]
    pub fn owner_of(&self, name: &str) -> Option<ToolOwner> {
        self.entries
            .iter()
            .any(|e| e.definition.name == name)
            .then_some(ToolOwner::Internal)
    }
}

impl Default for ToolCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ToolCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolCatalog")
            .field("static_entries", &self.entries.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Apply `ToolConfig` permission filters in-place.
fn apply_config_filter(tools: &mut Vec<ToolDefinition>, config: &ToolConfig) {
    const FS_TOOL_NAMES: &[&str] = &[
        "read_file",
        "write_file",
        "edit_file",
        "delete_file",
        "list_files",
        "find_files",
        "stat_file",
        "search_code",
    ];
    tools.retain(|t| {
        if !config.enable_commands && t.name == "run_command" {
            return false;
        }
        if !config.enable_fs && FS_TOOL_NAMES.contains(&t.name.as_str()) {
            return false;
        }
        true
    });
}

fn add_project_id_param(mut td: ToolDefinition) -> ToolDefinition {
    if let Some(props) = td
        .input_schema
        .get_mut("properties")
        .and_then(|p| p.as_object_mut())
    {
        props.insert(
            "project_id".to_string(),
            serde_json::json!({
                "type": "string",
                "description": "The project ID to operate on (required for multi-project context)"
            }),
        );
    }
    if let Some(req) = td.input_schema.get_mut("required") {
        if let Some(arr) = req.as_array_mut() {
            arr.insert(0, serde_json::json!("project_id"));
        }
    }
    td
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core::{InstalledToolDefinition, ToolAuth};

    #[test]
    fn catalog_has_entries() {
        let cat = ToolCatalog::new();
        assert!(cat.static_count() > 0);
    }

    #[test]
    fn core_profile_contains_fs_and_cmd() {
        let cat = ToolCatalog::new();
        let tools = cat.tools_for_profile(ToolProfile::Core);
        let names: HashSet<_> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains("read_file"));
        assert!(names.contains("write_file"));
        assert!(names.contains("run_command"));
        assert!(names.contains("search_code"));
    }

    #[test]
    fn agent_profile_includes_core_and_management() {
        let cat = ToolCatalog::new();
        let tools = cat.tools_for_profile(ToolProfile::Agent);
        let names: HashSet<_> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains("read_file"), "agent should include core");
        assert!(
            names.contains("list_specs"),
            "agent should include management"
        );
        assert!(
            !names.contains("task_done"),
            "agent should not include engine"
        );
    }

    #[test]
    fn engine_profile_includes_core_and_engine() {
        let cat = ToolCatalog::new();
        let tools = cat.tools_for_profile(ToolProfile::Engine);
        let names: HashSet<_> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains("read_file"), "engine should include core");
        assert!(
            names.contains("task_done"),
            "engine should include engine tools"
        );
        assert!(
            !names.contains("list_specs"),
            "engine should not include management"
        );
    }

    #[test]
    fn visible_tools_filters_by_config() {
        let cat = ToolCatalog::new();
        let mut config = ToolConfig::default();
        config.enable_commands = false;
        config.enable_fs = false;

        let tools = cat.visible_tools(ToolProfile::Core, &config);
        let names: HashSet<_> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(!names.contains("run_command"));
        assert!(!names.contains("read_file"));
    }

    #[test]
    fn owner_of_reports_correctly() {
        let cat = ToolCatalog::new();
        assert_eq!(cat.owner_of("read_file"), Some(ToolOwner::Internal));
        assert_eq!(cat.owner_of("nonexistent"), None);
    }

    #[test]
    fn multi_project_adds_project_id() {
        let cat = ToolCatalog::new();
        let config = ToolConfig::default();
        let tools = cat.visible_tools_multi_project(&config);
        for tool in &tools {
            let has_project_id = tool
                .input_schema
                .get("properties")
                .and_then(|p| p.get("project_id"))
                .is_some();
            assert!(
                has_project_id,
                "multi-project tool '{}' must have project_id",
                tool.name
            );
        }
    }

    #[test]
    fn no_duplicate_names_in_any_profile() {
        let cat = ToolCatalog::new();
        for profile in [ToolProfile::Core, ToolProfile::Agent, ToolProfile::Engine] {
            let tools = cat.tools_for_profile(profile);
            let mut seen = HashSet::new();
            for t in &tools {
                assert!(seen.insert(&t.name), "duplicate: {} in {profile:?}", t.name);
            }
        }
    }

    #[test]
    fn every_builtin_has_catalog_entry() {
        let cat = ToolCatalog::new();
        let core = cat.tools_for_profile(ToolProfile::Core);
        let names: HashSet<_> = core.iter().map(|t| t.name.as_str()).collect();
        for tool in builtin_tools() {
            assert!(
                names.contains(tool.name()),
                "builtin '{}' missing from core profile",
                tool.name()
            );
        }
    }

    #[test]
    fn with_installed_tools_adds_model_visible_tools_for_profile() {
        let cat = ToolCatalog::new();
        let extended = cat.with_installed_tools(
            ToolProfile::Engine,
            &[InstalledToolDefinition {
                name: "brave_search_web".to_string(),
                description: "Search the web using Brave".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": { "query": { "type": "string" } },
                    "required": ["query"]
                }),
                endpoint: "https://example.com/tool".to_string(),
                auth: ToolAuth::None,
                timeout_ms: None,
                namespace: None,
                required_integration: None,
                runtime_execution: None,
                metadata: Default::default(),
            }],
        );

        let tools = extended.tools_for_profile(ToolProfile::Engine);
        let names: HashSet<_> = tools.iter().map(|tool| tool.name.as_str()).collect();
        assert!(names.contains("brave_search_web"));
    }
}
