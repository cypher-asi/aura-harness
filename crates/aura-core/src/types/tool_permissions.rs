//! Per-tool permission types for the two-level (user default + per-agent
//! override) permission model.
//!
//! Every `(agent, tool)` resolves, via [`resolve_effective_permission`], to
//! exactly one [`ToolState`]: `on` / `off` / `ask` (Rust variants
//! [`ToolState::Allow`], [`ToolState::Deny`], [`ToolState::Ask`]). The
//! kernel policy gate is the single call site that consults this
//! resolution.
//!
//! Layer 1 — user-level default ([`UserToolDefaults`]): stored in RocksDB
//! keyed by the end-user id, editable via
//! `GET/PUT /users/:user_id/tool-defaults`. Three modes surfaced to
//! apps/clients:
//!
//!   * `FullAccess` — every tool resolves to `on`.
//!   * `AutoReview` — every tool resolves to `ask` (live WS prompt each
//!     time).
//!   * `DefaultPermissions { per_tool, fallback }` — the user's
//!     "standard permission set" applied to every agent they create.
//!     Each entry in `per_tool` is tri-state `on`/`off`/`ask`.
//!
//! Layer 2 — per-agent override ([`AgentToolPermissions`]): optional map
//! stored on [`crate::Identity`], stamped at spawn and editable via
//! `GET/PUT /agents/:agent_id/tool-permissions`. `None` (or empty map) =
//! inherit the user default verbatim. Present entries override per-tool
//! (tri-state); anything not in the map still flows through the user
//! default.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Resolved permission for a `(agent, tool)` pair. Tri-state: `on` / `off`
/// / `ask` on the wire; the Rust enum keeps precise internal names.
///
/// `#[serde(alias = "allow")]` / `#[serde(alias = "deny")]` exist purely
/// so any legacy JSON that leaked the internal spelling still parses;
/// newly produced JSON always uses the user-facing labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolState {
    /// `"on"` — execute without prompting.
    #[serde(rename = "on", alias = "allow")]
    Allow,
    /// `"off"` — reject the call at the policy gate; the kernel returns
    /// `PolicyViolation`.
    #[serde(rename = "off", alias = "deny")]
    Deny,
    /// `"ask"` — suspend the call and emit a `ToolApprovalPrompt` on the
    /// session WebSocket; the user's live response decides `on` or
    /// `off` for this invocation, with an optional `remember` scope to
    /// upgrade the persisted user default.
    #[serde(rename = "ask")]
    Ask,
}

impl ToolState {
    /// Monotonic permission ordering used for per-agent overrides:
    /// `off < ask < on`.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Deny => 0,
            Self::Ask => 1,
            Self::Allow => 2,
        }
    }

    /// Return whether `self` is no broader than `parent` under
    /// `off < ask < on`.
    #[must_use]
    pub const fn is_subset_of(self, parent: Self) -> bool {
        self.rank() <= parent.rank()
    }
}

/// User-scoped default permissions applied to every agent owned by that user
/// (subject to optional per-agent overrides).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserToolDefaults {
    pub mode: UserDefaultMode,
}

impl UserToolDefaults {
    /// "Default ON" — every tool allowed. Used when a user has no persisted
    /// entry yet (first-run).
    #[must_use]
    pub fn full_access() -> Self {
        Self {
            mode: UserDefaultMode::FullAccess,
        }
    }

    #[must_use]
    pub fn auto_review() -> Self {
        Self {
            mode: UserDefaultMode::AutoReview,
        }
    }

    #[must_use]
    pub fn default_permissions(per_tool: BTreeMap<String, ToolState>, fallback: ToolState) -> Self {
        Self {
            mode: UserDefaultMode::DefaultPermissions { per_tool, fallback },
        }
    }
}

impl Default for UserToolDefaults {
    fn default() -> Self {
        Self::full_access()
    }
}

/// The three client-facing modes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UserDefaultMode {
    /// Every tool resolves to [`ToolState::Allow`].
    FullAccess,
    /// Every tool resolves to [`ToolState::Ask`].
    AutoReview,
    /// User-defined per-tool map with a `fallback` for tools not in the map.
    /// This is the "Default Permissions" mode in the UX — the user's
    /// standard permission set.
    DefaultPermissions {
        per_tool: BTreeMap<String, ToolState>,
        fallback: ToolState,
    },
}

/// Per-agent override map. `None` on [`crate::Identity`] (or an empty map
/// here) means "inherit the user default for every tool". Populated entries
/// override only that specific tool; anything not listed still flows through
/// the user default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentToolPermissions {
    #[serde(default)]
    pub per_tool: BTreeMap<String, ToolState>,
}

impl AgentToolPermissions {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with(mut self, tool: impl Into<String>, state: ToolState) -> Self {
        self.per_tool.insert(tool.into(), state);
        self
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.per_tool.is_empty()
    }
}

/// Resolve the effective [`ToolState`] for a given tool, consulting the
/// per-agent override first and falling back to the user default.
#[must_use]
pub fn resolve_effective_permission(
    user_default: &UserToolDefaults,
    agent_override: Option<&AgentToolPermissions>,
    tool: &str,
) -> ToolState {
    if let Some(state) = agent_override.and_then(|o| o.per_tool.get(tool)) {
        return *state;
    }
    match &user_default.mode {
        UserDefaultMode::FullAccess => ToolState::Allow,
        UserDefaultMode::AutoReview => ToolState::Ask,
        UserDefaultMode::DefaultPermissions { per_tool, fallback } => {
            per_tool.get(tool).copied().unwrap_or(*fallback)
        }
    }
}

/// Return whether the current user/agent tool policy is globally full access.
///
/// This is intentionally stricter than asking whether one specific tool
/// resolves to [`ToolState::Allow`]. A session only counts as effectively full
/// access when the user default is FullAccess and the per-agent override does
/// not narrow any named tool to `ask` or `off`.
#[must_use]
pub fn is_effectively_full_access(
    user_default: &UserToolDefaults,
    agent_override: Option<&AgentToolPermissions>,
) -> bool {
    matches!(user_default.mode, UserDefaultMode::FullAccess)
        && agent_override
            .map(|override_permissions| {
                override_permissions
                    .per_tool
                    .values()
                    .all(|state| *state == ToolState::Allow)
            })
            .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom(per_tool: &[(&str, ToolState)], fallback: ToolState) -> UserToolDefaults {
        UserToolDefaults::default_permissions(
            per_tool
                .iter()
                .map(|(k, v)| ((*k).to_string(), *v))
                .collect(),
            fallback,
        )
    }

    fn override_with(entries: &[(&str, ToolState)]) -> AgentToolPermissions {
        let mut out = AgentToolPermissions::new();
        for (k, v) in entries {
            out.per_tool.insert((*k).to_string(), *v);
        }
        out
    }

    #[test]
    fn full_access_mode_allows_everything_without_override() {
        let user = UserToolDefaults::full_access();
        for tool in ["read_file", "run_command", "whatever"] {
            assert_eq!(
                resolve_effective_permission(&user, None, tool),
                ToolState::Allow
            );
        }
    }

    #[test]
    fn auto_review_mode_asks_everything_without_override() {
        let user = UserToolDefaults::auto_review();
        for tool in ["read_file", "run_command", "whatever"] {
            assert_eq!(
                resolve_effective_permission(&user, None, tool),
                ToolState::Ask
            );
        }
    }

    #[test]
    fn default_permissions_consults_per_tool_then_fallback() {
        let user = custom(
            &[
                ("read_file", ToolState::Allow),
                ("run_command", ToolState::Ask),
            ],
            ToolState::Deny,
        );
        assert_eq!(
            resolve_effective_permission(&user, None, "read_file"),
            ToolState::Allow
        );
        assert_eq!(
            resolve_effective_permission(&user, None, "run_command"),
            ToolState::Ask
        );
        assert_eq!(
            resolve_effective_permission(&user, None, "not_in_map"),
            ToolState::Deny
        );
    }

    #[test]
    fn agent_override_wins_over_user_default() {
        let user = UserToolDefaults::full_access();
        let ov = override_with(&[("run_command", ToolState::Deny)]);
        assert_eq!(
            resolve_effective_permission(&user, Some(&ov), "run_command"),
            ToolState::Deny
        );
        assert_eq!(
            resolve_effective_permission(&user, Some(&ov), "read_file"),
            ToolState::Allow,
            "unlisted tool still flows through user default"
        );
    }

    #[test]
    fn empty_override_map_is_equivalent_to_none() {
        let user = UserToolDefaults::auto_review();
        let empty = AgentToolPermissions::new();
        assert_eq!(
            resolve_effective_permission(&user, Some(&empty), "read_file"),
            resolve_effective_permission(&user, None, "read_file"),
        );
    }

    #[test]
    fn effective_full_access_requires_full_access_user_default() {
        assert!(is_effectively_full_access(
            &UserToolDefaults::full_access(),
            None
        ));
        assert!(!is_effectively_full_access(
            &UserToolDefaults::auto_review(),
            None
        ));
        assert!(!is_effectively_full_access(
            &custom(&[], ToolState::Allow),
            None
        ));
    }

    #[test]
    fn effective_full_access_allows_empty_or_allow_only_overrides() {
        let user = UserToolDefaults::full_access();
        assert!(is_effectively_full_access(
            &user,
            Some(&AgentToolPermissions::new())
        ));
        assert!(is_effectively_full_access(
            &user,
            Some(&override_with(&[
                ("read_file", ToolState::Allow),
                ("run_command", ToolState::Allow),
            ]))
        ));
    }

    #[test]
    fn effective_full_access_rejects_narrowing_overrides() {
        let user = UserToolDefaults::full_access();
        assert!(!is_effectively_full_access(
            &user,
            Some(&override_with(&[("run_command", ToolState::Ask)]))
        ));
        assert!(!is_effectively_full_access(
            &user,
            Some(&override_with(&[("read_file", ToolState::Deny)]))
        ));
    }

    #[test]
    fn override_can_promote_and_demote() {
        let user = custom(&[("run_command", ToolState::Deny)], ToolState::Allow);

        let promote = override_with(&[("run_command", ToolState::Allow)]);
        assert_eq!(
            resolve_effective_permission(&user, Some(&promote), "run_command"),
            ToolState::Allow,
        );

        let demote = override_with(&[("read_file", ToolState::Deny)]);
        assert_eq!(
            resolve_effective_permission(&user, Some(&demote), "read_file"),
            ToolState::Deny,
        );
    }

    #[test]
    fn ask_state_survives_through_both_layers() {
        let user = custom(&[("run_command", ToolState::Ask)], ToolState::Allow);
        assert_eq!(
            resolve_effective_permission(&user, None, "run_command"),
            ToolState::Ask,
        );

        let ov = override_with(&[("read_file", ToolState::Ask)]);
        assert_eq!(
            resolve_effective_permission(&UserToolDefaults::full_access(), Some(&ov), "read_file"),
            ToolState::Ask,
        );
    }

    #[test]
    fn roundtrip_user_defaults_serde() {
        for user in [
            UserToolDefaults::full_access(),
            UserToolDefaults::auto_review(),
            custom(&[("read_file", ToolState::Allow)], ToolState::Deny),
        ] {
            let json = serde_json::to_string(&user).unwrap();
            let parsed: UserToolDefaults = serde_json::from_str(&json).unwrap();
            assert_eq!(user, parsed);
        }
    }

    #[test]
    fn roundtrip_agent_tool_permissions_serde() {
        let ov = override_with(&[
            ("read_file", ToolState::Allow),
            ("run_command", ToolState::Deny),
            ("write_file", ToolState::Ask),
        ]);
        let json = serde_json::to_string(&ov).unwrap();
        let parsed: AgentToolPermissions = serde_json::from_str(&json).unwrap();
        assert_eq!(ov, parsed);
    }

    #[test]
    fn first_run_user_default_is_full_access() {
        assert_eq!(UserToolDefaults::default(), UserToolDefaults::full_access());
    }

    #[test]
    fn tool_state_serialises_as_on_off_ask() {
        assert_eq!(serde_json::to_string(&ToolState::Allow).unwrap(), "\"on\"");
        assert_eq!(serde_json::to_string(&ToolState::Deny).unwrap(), "\"off\"");
        assert_eq!(serde_json::to_string(&ToolState::Ask).unwrap(), "\"ask\"");
    }

    #[test]
    fn tool_state_deserialises_user_facing_labels() {
        assert_eq!(
            serde_json::from_str::<ToolState>("\"on\"").unwrap(),
            ToolState::Allow
        );
        assert_eq!(
            serde_json::from_str::<ToolState>("\"off\"").unwrap(),
            ToolState::Deny
        );
        assert_eq!(
            serde_json::from_str::<ToolState>("\"ask\"").unwrap(),
            ToolState::Ask
        );
    }

    #[test]
    fn tool_state_aliases_accept_legacy_allow_deny_spellings() {
        assert_eq!(
            serde_json::from_str::<ToolState>("\"allow\"").unwrap(),
            ToolState::Allow
        );
        assert_eq!(
            serde_json::from_str::<ToolState>("\"deny\"").unwrap(),
            ToolState::Deny
        );
    }

    #[test]
    fn agent_tool_permissions_wire_shape_uses_tri_state() {
        let ov = override_with(&[
            ("read_file", ToolState::Allow),
            ("run_command", ToolState::Ask),
            ("delete_file", ToolState::Deny),
        ]);
        let json = serde_json::to_string(&ov).unwrap();
        assert!(json.contains("\"on\""), "expected \"on\" in {json}");
        assert!(json.contains("\"off\""), "expected \"off\" in {json}");
        assert!(json.contains("\"ask\""), "expected \"ask\" in {json}");
    }
}
