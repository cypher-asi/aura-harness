//! Canonical tool catalog — single source of truth for tool metadata.
//!
//! Stores all tool entries (internal built-ins and schema-only definitions)
//! with profile and owner annotations.

use crate::definitions;
use crate::tool::builtin_tools;
use crate::ToolConfig;
use aura_core_types::{
    AgentPermissions, Capability, InstalledToolDefinition, Registry, RegistryError, ToolDefinition,
};
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
    /// Phase 5: capabilities the caller must hold for this tool to be
    /// visible + callable. Empty means universally visible (no capability
    /// gate).
    pub required_capabilities: Vec<Capability>,
}

/// Collects catalog entries while tracking which names are taken.
#[derive(Default)]
struct CatalogBuilder {
    entries: Vec<CatalogEntry>,
    seen: HashSet<String>,
}

impl CatalogBuilder {
    /// Add an internal tool, recording its name.
    fn add(
        &mut self,
        definition: ToolDefinition,
        profiles: Vec<ToolProfile>,
        required_capabilities: Vec<Capability>,
    ) {
        self.seen.insert(definition.name.clone());
        self.entries.push(CatalogEntry {
            definition,
            owner: ToolOwner::Internal,
            profiles,
            required_capabilities,
        });
    }

    /// Add the internal tool `entry` builds unless `name` is already in
    /// the catalog.
    fn add_unless_seen(
        &mut self,
        name: String,
        profiles: Vec<ToolProfile>,
        entry: impl FnOnce() -> (ToolDefinition, Vec<Capability>),
    ) {
        if self.seen.insert(name) {
            let (definition, required_capabilities) = entry();
            self.entries.push(CatalogEntry {
                definition,
                owner: ToolOwner::Internal,
                profiles,
                required_capabilities,
            });
        }
    }
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
        let all_profiles = vec![ToolProfile::Core, ToolProfile::Agent, ToolProfile::Engine];
        let mut catalog = CatalogBuilder::default();

        // Core tools from curated definitions (model-facing schemas).
        for def in definitions::core_tool_definitions() {
            catalog.add(def, all_profiles.clone(), Vec::new());
        }

        // Built-in tool impls not covered by definitions.rs (e.g. stat_file).
        for tool in builtin_tools() {
            catalog.add_unless_seen(tool.name().to_string(), all_profiles.clone(), || {
                (tool.definition(), tool.required_capabilities())
            });
        }

        // Cross-agent tools. Always compiled + registered, but
        // `visible_tools` drops them for callers that don't hold the matching
        // capabilities, so low-privilege agents never see these tool names in
        // their prompt. The kernel's policy gate is always on and enforces
        // the same capabilities at proposal time regardless of visibility.
        for (_tool, definition, required) in crate::agents::cross_agent_catalog_entries() {
            catalog.add_unless_seen(definition.name.clone(), vec![ToolProfile::Agent], || {
                (definition, required)
            });
        }

        // Computer-use tool. Always compiled into the catalog but gated
        // behind `Capability::ComputerUse`, so `visible_tools` hides it
        // from runs that did not opt into computer-use. The executable
        // `ComputerTool` impl (which needs the per-session executor URL)
        // is registered separately on the session's tool resolver.
        catalog.add_unless_seen(
            crate::computer_tool::COMPUTER_TOOL_NAME.to_string(),
            all_profiles,
            || {
                (
                    crate::computer_tool::computer_tool_definition(),
                    vec![Capability::ComputerUse],
                )
            },
        );

        // Agent-only management tools (spec, task, project, dev-loop).
        for def in definitions::chat_management_tools() {
            let required_capabilities = domain_tool_required_capabilities(&def.name);
            catalog.add(def, vec![ToolProfile::Agent], required_capabilities);
        }

        // Engine-only tools (task_done, get_task_context, submit_plan).
        for def in definitions::engine_specific_tools() {
            catalog.add(def, vec![ToolProfile::Engine], Vec::new());
        }

        let entries = catalog.entries;
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
            .filter(|e| e.required_capabilities.is_empty())
            .map(|e| e.definition.clone())
            .collect()
    }

    /// Built-in tool definitions backed by [`ToolExecutor`](crate::ToolExecutor).
    ///
    /// This follows [`builtin_tools`] order and looks up each definition in the
    /// catalog so executor bootstrap callers do not advertise schema-only tools
    /// that their router cannot dispatch.
    #[must_use]
    pub fn executor_builtin_tools(&self) -> Vec<ToolDefinition> {
        builtin_tools()
            .into_iter()
            .filter_map(|tool| {
                let name = tool.name().to_string();
                self.entries
                    .iter()
                    .find(|entry| entry.definition.name == name)
                    .map(|entry| entry.definition.clone())
            })
            .collect()
    }

    /// Phase 5: like [`Self::tools_for_profile`] but additionally includes
    /// capability-gated tools when `permissions` holds every required
    /// capability. When `permissions` is `None` the result matches
    /// [`Self::tools_for_profile`] exactly (capability-gated tools hidden).
    #[must_use]
    pub fn tools_for_profile_with_permissions(
        &self,
        profile: ToolProfile,
        permissions: Option<&AgentPermissions>,
    ) -> Vec<ToolDefinition> {
        self.entries
            .iter()
            .filter(|e| e.profiles.contains(&profile))
            .filter(|e| entry_visible(e, permissions))
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
                required_capabilities: Vec::new(),
            });
        }

        Self { entries }
    }

    /// Get visible tools for a profile.
    ///
    /// Equivalent to [`Self::visible_tools_with_permissions`] with
    /// `permissions == None`: capability-gated tools (like `spawn_agent`) are
    /// hidden. Callers that can supply the caller's [`AgentPermissions`]
    /// should prefer the `_with_permissions` variant.
    #[must_use]
    pub fn visible_tools(&self, profile: ToolProfile, _config: &ToolConfig) -> Vec<ToolDefinition> {
        self.visible_tools_with_permissions(profile, _config, None)
    }

    /// Phase 5: visible tools for a profile, filtered by the caller's
    /// capability grants.
    #[must_use]
    pub fn visible_tools_with_permissions(
        &self,
        profile: ToolProfile,
        _config: &ToolConfig,
        permissions: Option<&AgentPermissions>,
    ) -> Vec<ToolDefinition> {
        self.tools_for_profile_with_permissions(profile, permissions)
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

fn domain_tool_required_capabilities(name: &str) -> Vec<Capability> {
    match name {
        "post_to_feed" => vec![Capability::PostToFeed],
        "check_budget" | "record_usage" => vec![Capability::ManageBilling],
        _ => Vec::new(),
    }
}

impl Default for ToolCatalog {
    fn default() -> Self {
        Self::new()
    }
}

/// `Registry` trait impl (Wave 4 unification).
///
/// `ToolCatalog` is an immutable, compile-time–constructed store: entries
/// are populated once by [`ToolCatalog::new`] from the bundled definitions
/// and there is no runtime-mutable insert path. The `register` /
/// `remove` methods therefore return [`RegistryError::Unsupported`] /
/// `None` respectively. `get` / `iter` / `len` expose the read-only
/// name -> [`CatalogEntry`] view shared with `SkillRegistry` and
/// `AutomatonRuntime`.
///
/// Callers that need profile- or capability-filtered views should
/// continue to use the inherent [`ToolCatalog::visible_tools`] and
/// friends — the trait impl deliberately exposes the raw catalog shape.
impl Registry for ToolCatalog {
    type Id = String;
    type Item = CatalogEntry;

    fn register(&mut self, _id: Self::Id, _item: Self::Item) -> Result<(), RegistryError> {
        Err(RegistryError::Unsupported(
            "ToolCatalog is immutable; construct via ToolCatalog::new or with_installed_tools",
        ))
    }

    fn get(&self, id: &Self::Id) -> Option<Self::Item> {
        self.entries
            .iter()
            .find(|entry| &entry.definition.name == id)
            .cloned()
    }

    fn iter(&self) -> Vec<(Self::Id, Self::Item)> {
        self.entries
            .iter()
            .map(|entry| (entry.definition.name.clone(), entry.clone()))
            .collect()
    }

    fn remove(&mut self, _id: &Self::Id) -> Option<Self::Item> {
        None
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
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

/// Phase 5: decide whether a catalog entry is visible to a caller with the
/// given optional [`AgentPermissions`]. Entries with no
/// `required_capabilities` are always visible; otherwise every required
/// capability must be satisfied by some capability in the bundle.
///
/// "Satisfied" runs through [`Capability::satisfies`], so project wildcards
/// ([`Capability::ReadAllProjects`] / [`Capability::WriteAllProjects`]) on
/// the bundle cover any exact-id `ReadProject { id }` / `WriteProject { id }`
/// requirement declared by a tool. This matches the server-side policy in
/// `aura-os-agent-tools::permissions_satisfy_requirements` so a CEO bundle
/// shipped over the wire doesn't silently lose project-scoped tools on the
/// harness side.
fn entry_visible(entry: &CatalogEntry, permissions: Option<&AgentPermissions>) -> bool {
    if entry.required_capabilities.is_empty() {
        return true;
    }
    let Some(permissions) = permissions else {
        return false;
    };
    entry.required_capabilities.iter().all(|req| {
        permissions
            .capabilities
            .iter()
            .any(|held| held.satisfies(req))
    })
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
mod tests;
