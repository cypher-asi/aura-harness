//! Plugin enable/disable + per-plugin overrides table (Phase 4a).
//!
//! The on-disk shape is a TOML table keyed by plugin name. There is
//! intentionally no env-var override surface today; plugin policy
//! lives in the operator's config file (`~/.aura/config.toml` in the
//! Phase 4b layout).
//!
//! ## Invariants ([rules.md §13])
//!
//! - An empty `[plugins]` table => zero plugins active. A user with
//!   no config file sees the same "no third-party hooks" behaviour
//!   they have today.
//! - [`PluginConfig::trusted`] defaults to `false`. A plugin MUST be
//!   explicitly trusted before its hooks fire (the `aura-plugin-hooks`
//!   crate, landing in Phase 4b, enforces this).
//! - The wrapping [`PluginsConfig`] is `defaults() const fn` so the
//!   root [`crate::AuraConfig::defaults`] stays const-evaluable.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Plugin section root. Wraps a name-keyed [`PluginsTable`].
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, rename_all = "snake_case")]
pub struct PluginsConfig {
    /// Per-plugin entries keyed by plugin name.
    pub table: PluginsTable,
}

impl PluginsConfig {
    /// Compile-time defaults (empty table).
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            table: PluginsTable::empty(),
        }
    }
}

/// Transparent newtype around the per-plugin map.
///
/// Modelled as `transparent` so a TOML `[plugins]` table maps
/// directly to this type without an intermediate `{ entries = {...} }`
/// wrapper.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct PluginsTable(pub BTreeMap<String, PluginConfig>);

impl PluginsTable {
    /// Empty table. `const fn` so [`PluginsConfig::defaults`] can
    /// remain const-evaluable.
    #[must_use]
    pub const fn empty() -> Self {
        Self(BTreeMap::new())
    }
}

/// Per-plugin config row. Defaults to `enabled = false, trusted = false`
/// so a freshly-installed plugin is inert until the operator explicitly
/// turns it on.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, rename_all = "snake_case")]
pub struct PluginConfig {
    /// When `false`, the plugin is loaded into the registry but its
    /// hooks are not invoked.
    pub enabled: bool,
    /// When `false`, security-gated hook surfaces (filesystem writes,
    /// command execution) skip this plugin even when
    /// `enabled = true`. See plan §4 for the full gating matrix.
    pub trusted: bool,
    /// Operator-pinned plugin version. `None` means "use whatever
    /// `aura-plugin-core` resolves".
    pub version: Option<String>,
}

impl PluginConfig {
    /// Compile-time defaults. `const fn` to satisfy the root
    /// const-evaluability guard.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            enabled: false,
            trusted: false,
            version: None,
        }
    }
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_empty_and_inert() {
        let cfg = PluginsConfig::defaults();
        assert!(cfg.table.0.is_empty(), "plugins table must default empty");
    }

    #[test]
    fn defaults_are_const_evaluable() {
        const _PLUGINS: PluginsConfig = PluginsConfig::defaults();
        const _ROW: PluginConfig = PluginConfig::defaults();
    }

    #[test]
    fn plugin_defaults_are_inert() {
        let cfg = PluginConfig::defaults();
        assert!(!cfg.enabled);
        assert!(!cfg.trusted);
        assert!(cfg.version.is_none());
    }

    #[test]
    fn round_trips_through_json() {
        let mut table = BTreeMap::new();
        table.insert(
            "alpha".to_string(),
            PluginConfig {
                enabled: true,
                trusted: false,
                version: Some("1.2.3".to_string()),
            },
        );
        let cfg = PluginsConfig {
            table: PluginsTable(table),
        };
        let json = serde_json::to_string(&cfg).expect("serialize");
        let restored: PluginsConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cfg, restored);
    }
}
