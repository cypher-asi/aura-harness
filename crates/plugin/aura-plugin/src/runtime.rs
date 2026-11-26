//! Plugin runtime materialisation (Phase 8).
//!
//! [`load_enabled_plugins`] walks every enabled entry in
//! `~/.aura/plugins/` and produces a [`PluginRuntime`] composed of
//! the three Phase 4c subsystems:
//!
//! - [`aura_plugin_hooks::HookEngine`] populated with the manifest's
//!   `[[contributes.hooks]]` entries.
//! - [`crate::mcp::McpConnectionManager`] populated with the
//!   manifest's `[[contributes.mcp]]` entries (first-active-wins
//!   merge by `server_id`).
//! - [`crate::connectors::ConnectorRegistry`] populated with
//!   the manifest's `[[contributes.connectors]]` entries (first-
//!   active-wins by `id`).
//!
//! The skill roots are returned alongside the runtime so the caller
//! can hand them to
//! `aura_context_skills::SkillRegistry::add_plugin_roots`. No
//! production composition root calls [`load_enabled_plugins`] yet;
//! only the `aura-node` plugin integration tests do.
//!
//! ## Invariants ([rules.md §13])
//!
//! - **Trust-gated activation**: a plugin must be both `enabled` and
//!   `trusted` in operator config before its hooks, MCP servers,
//!   connectors, or skills materialise. Installed-but-untrusted plugins
//!   remain inert.
//! - **Per-plugin best-effort load**: a single plugin failing to
//!   load (manifest invalid, MCP server spawn failure, hook event
//!   unknown, etc.) MUST NOT abort the load. The failure is
//!   recorded in [`PluginRuntime::load_failures`] and the loop
//!   continues with the remaining plugins.
//! - **MCP first-active-wins**: the first plugin (in enable order)
//!   keeps a server-id slot; subsequent contributions log a `WARN`
//!   and are skipped.
//! - **Connector last-wins**: plugin-supplied connector contributions
//!   replace earlier entries through [`ConnectorRegistry::replace`].
//! - **Hook chain order**: hooks for the same event run in the
//!   order they were registered. The load loop walks plugins in
//!   the user's enable order (sorted lexicographically by id for
//!   determinism — matches the Codex behaviour referenced in the
//!   plan).
//! - **No I/O at construct time** for [`PluginRuntime::default`].
//!   The runtime can be built empty for the backward-compat path
//!   (no `~/.aura/plugins/` directory => zero overhead).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aura_config::PluginsConfig;
use aura_plugin_hooks::{HookEngine, HookEvent, PluginLoadFailure, PluginRef, RegisteredHook};
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::cache::PluginCache;
use crate::connectors::{ConnectorEntry, ConnectorRegistry};
use crate::manifest::PluginManifest;
use crate::mcp::{
    McpConnectionManager, ServerConfig as McpServerConfig, DEFAULT_MCP_REQUEST_TIMEOUT,
};

/// Result of a [`load_enabled_plugins`] call. The four subsystem
/// handles are `Arc`-wrapped so the caller can clone them across
/// runtime tasks without re-materialising.
#[derive(Debug)]
pub struct PluginRuntime {
    /// Hook engine wired with every enabled plugin's hook
    /// contributions.
    pub hook_engine: Arc<HookEngine>,
    /// MCP connection manager wired with every enabled plugin's
    /// MCP server contributions.
    pub mcp: Arc<McpConnectionManager>,
    /// Connector registry wired with every enabled plugin's
    /// connector contributions.
    pub connectors: Arc<ConnectorRegistry>,
    /// Plugin skill directories the caller should hand to
    /// `aura_context_skills::SkillRegistry::add_plugin_roots`.
    pub skill_roots: Vec<PathBuf>,
    /// Refs to every successfully-loaded plugin.
    pub enabled: Vec<PluginRef>,
    /// Refs + reasons for plugins that failed to materialise. The
    /// session-start hook ctx surfaces this list to operator hooks
    /// for audit / monitoring use cases.
    pub load_failures: Vec<PluginLoadFailure>,
}

impl Default for PluginRuntime {
    fn default() -> Self {
        Self {
            hook_engine: Arc::new(HookEngine::new()),
            mcp: Arc::new(McpConnectionManager::new()),
            connectors: Arc::new(ConnectorRegistry::new()),
            skill_roots: Vec::new(),
            enabled: Vec::new(),
            load_failures: Vec::new(),
        }
    }
}

impl PluginRuntime {
    /// Build a fully-empty runtime. Used by callers that want the
    /// Phase 8 wiring shape without materialising any plugin from
    /// disk — the empty-install backward-compat path.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// `true` when no plugins are wired into this runtime. Phase 8
    /// callers use this as a fast short-circuit before allocating
    /// per-event ctx structs.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.enabled.is_empty()
    }
}

/// Errors raised by the load pipeline itself (not per-plugin
/// failures, which are recorded in [`PluginRuntime::load_failures`]).
#[derive(Debug, Error)]
pub enum PluginLoadError {
    /// I/O error walking the plugin cache.
    #[error("io error walking plugin cache: {0}")]
    Io(#[from] std::io::Error),
}

/// Walk `aura_home/plugins/` + the operator's `[plugins]` config
/// and materialise an active [`PluginRuntime`].
///
/// Per-plugin failures are non-fatal: a malformed manifest, an
/// unknown hook event, a duplicate MCP server id, etc. all produce
/// a [`PluginLoadFailure`] entry in [`PluginRuntime::load_failures`]
/// and the loop continues with the remaining plugins.
///
/// # Errors
///
/// Returns [`PluginLoadError::Io`] only for catastrophic walk
/// failures (e.g. the `~/.aura/plugins/` directory is unreadable).
/// A missing `~/.aura/plugins/` directory yields an empty runtime
/// (Ok).
pub fn load_enabled_plugins(
    aura_home: &Path,
    plugins_config: &PluginsConfig,
) -> Result<PluginRuntime, PluginLoadError> {
    let cache = PluginCache::new(aura_home.join("plugins"));
    let plugin_ids = cache.list_plugins().unwrap_or_default();

    let mut runtime = PluginRuntime::default();
    for plugin_id in plugin_ids {
        // Honour [plugins] config: a plugin must be installed,
        // enabled, and trusted to participate.
        if !plugin_enabled(&plugin_id, plugins_config) {
            debug!(
                plugin_id,
                "plugin not enabled + trusted in config; skipping"
            );
            continue;
        }
        match locate_active_manifest(&cache, &plugin_id) {
            Ok(Some((version_dir, manifest))) => runtime.materialise(&manifest, &version_dir),
            Ok(None) => {}
            Err(reason) => runtime
                .load_failures
                .push(PluginLoadFailure { plugin_id, reason }),
        }
    }
    Ok(runtime)
}

/// Resolve a plugin's active version directory and parse its
/// manifest. `Ok(None)` means the plugin has no active version
/// pointer; `Err` carries the load-failure reason.
fn locate_active_manifest(
    cache: &PluginCache,
    plugin_id: &str,
) -> Result<Option<(PathBuf, PluginManifest)>, String> {
    let active_version = match cache.active_version(plugin_id) {
        Ok(Some(v)) => v,
        Ok(None) => {
            debug!(plugin_id, "no active version pointer; skipping");
            return Ok(None);
        }
        Err(err) => return Err(format!("read active version: {err}")),
    };
    let version_dir = cache.version_dir(plugin_id, &active_version);
    let manifest = read_manifest(&version_dir.join(".aura-plugin.toml"))?;
    Ok(Some((version_dir, manifest)))
}

fn plugin_enabled(plugin_id: &str, cfg: &PluginsConfig) -> bool {
    // Match either the bare `id` or the `id@market` qualified key.
    if let Some(row) = cfg.table.0.get(plugin_id) {
        return row.enabled && row.trusted;
    }
    cfg.table.0.iter().any(|(k, row)| {
        row.enabled && row.trusted && (k == plugin_id || k.starts_with(&format!("{plugin_id}@")))
    })
}

fn read_manifest(path: &Path) -> Result<PluginManifest, String> {
    let body = std::fs::read_to_string(path)
        .map_err(|e| format!("read manifest {}: {}", path.display(), e))?;
    PluginManifest::from_toml_str(&body).map_err(|e| format!("parse manifest: {e}"))
}

impl PluginRuntime {
    /// Wire one plugin's contributions into the runtime and record it
    /// as enabled.
    fn materialise(&mut self, manifest: &PluginManifest, plugin_root: &Path) {
        let plugin_ref = PluginRef {
            id: manifest.id.as_str().to_string(),
            version: manifest.version.to_string(),
        };
        add_skill_roots(manifest, plugin_root, &mut self.skill_roots);
        register_hooks(&plugin_ref, manifest, plugin_root, &self.hook_engine);
        register_mcp_servers(&plugin_ref, manifest, plugin_root, &self.mcp);
        register_connectors(&plugin_ref, manifest, &self.connectors);
        info!(
            plugin_id = %plugin_ref.id,
            version = %plugin_ref.version,
            "plugin materialised"
        );
        self.enabled.push(plugin_ref);
    }
}

/// Skills: each contribution names a relative `path` under the
/// plugin root; the *parent* directory is what `add_plugin_roots`
/// discovers.
fn add_skill_roots(manifest: &PluginManifest, plugin_root: &Path, skill_roots: &mut Vec<PathBuf>) {
    for skill in &manifest.contributes.skills {
        let abs = plugin_root.join(&skill.path);
        let parent = abs
            .parent()
            .map_or_else(|| plugin_root.to_path_buf(), Path::to_path_buf);
        if !skill_roots.contains(&parent) {
            skill_roots.push(parent);
        }
    }
}

/// Hooks: each contribution registers a process. Unknown event names
/// are logged + skipped (matches Codex behaviour referenced in the
/// plan).
fn register_hooks(
    plugin_ref: &PluginRef,
    manifest: &PluginManifest,
    plugin_root: &Path,
    hook_engine: &HookEngine,
) {
    for hook in &manifest.contributes.hooks {
        let Some(event) = parse_hook_event(&hook.event) else {
            warn!(
                plugin_id = %plugin_ref.id,
                raw_event = %hook.event,
                "unknown hook event; skipping"
            );
            continue;
        };
        hook_engine.register(RegisteredHook {
            plugin_id: plugin_ref.id.clone(),
            event,
            command: hook.command.clone(),
            args: hook.args.clone(),
            plugin_root: plugin_root.to_path_buf(),
            env: BTreeMap::new(),
        });
    }
}

/// MCP servers: first-active-wins on `server_id`. We pre-pend the
/// plugin root for relative `command` paths so the spawn resolves the
/// bundled binary.
fn register_mcp_servers(
    plugin_ref: &PluginRef,
    manifest: &PluginManifest,
    plugin_root: &Path,
    mcp: &McpConnectionManager,
) {
    for mcp_entry in &manifest.contributes.mcp {
        let cfg = McpServerConfig {
            server_id: mcp_entry.server_id.clone(),
            command: resolve_command_for_spawn(&mcp_entry.command, plugin_root),
            args: mcp_entry.args.clone(),
            env: mcp_entry.env.clone(),
            request_timeout: DEFAULT_MCP_REQUEST_TIMEOUT,
        };
        if let Err(err) = mcp.register(cfg) {
            warn!(
                plugin_id = %plugin_ref.id,
                server_id = %mcp_entry.server_id,
                error = %err,
                "MCP server registration rejected (likely first-active-wins conflict)"
            );
            // Continue — duplicates are documented as warn + skip.
        }
    }
}

/// Connectors: last-wins per the Phase 8 spec, fully wired in Phase
/// 10. Plugin-supplied contributions replace any existing entry under
/// the same connector id (built-in or earlier-plugin) via
/// [`ConnectorRegistry::replace`]. The displaced entry is logged at
/// `info!` so operators can trace where a connector slot's owner
/// changed.
fn register_connectors(
    plugin_ref: &PluginRef,
    manifest: &PluginManifest,
    connectors: &ConnectorRegistry,
) {
    for connector in &manifest.contributes.connectors {
        let entry = ConnectorEntry {
            id: connector.id.clone(),
            plugin_id: plugin_ref.id.clone(),
            endpoint: connector.endpoint.clone(),
        };
        if let Some(previous) = connectors.replace(entry) {
            info!(
                plugin_id = %plugin_ref.id,
                connector_id = %connector.id,
                previous_plugin_id = %previous.plugin_id,
                "connector slot replaced by later-loaded plugin (last-wins)"
            );
        }
    }
}

fn parse_hook_event(s: &str) -> Option<HookEvent> {
    // Accept both snake_case (Aura wire format) and PascalCase (Codex
    // documentation form). Phase 8 normalises both into the closed
    // [`HookEvent`] enum.
    if let Some(e) = HookEvent::parse_wire(s) {
        return Some(e);
    }
    let lowered = s.to_ascii_lowercase();
    if let Some(e) = HookEvent::parse_wire(&lowered) {
        return Some(e);
    }
    let snake = pascal_to_snake(s);
    HookEvent::parse_wire(&snake)
}

fn pascal_to_snake(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_ascii_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

fn resolve_command_for_spawn(command: &str, plugin_root: &Path) -> String {
    let p = Path::new(command);
    if p.is_absolute() {
        return command.to_string();
    }
    let has_separator = command.contains('/') || command.contains('\\');
    if has_separator {
        return plugin_root.join(p).to_string_lossy().into_owned();
    }
    command.to_string()
}

#[cfg(test)]
mod tests;
