//! Agent permission primitives.
//!
//! Introduced by phase 5 of the super-agent / harness unification plan.
//! `AgentPermissions` bundles an [`AgentScope`] (which orgs / projects / agent
//! ids this caller may touch) with a set of [`Capability`] grants. The types
//! live in `aura-core` so both the kernel (policy gate) and the tools crate
//! (capability-gated tool registration) can reference them without pulling in
//! a larger dependency.
//!
//! `None` permissions on an agent record means "legacy — no explicit grants",
//! which phase 6's migrator will backfill with [`AgentPermissions::legacy_default`].

use serde::{Deserialize, Serialize};

/// Capabilities an agent can hold. Enforced by [`crate::ActionKind::Delegate`]
/// proposals at the kernel layer (phase 5+) and used to gate visibility of
/// cross-agent tools in the catalog.
///
/// Serialized as an externally-tagged enum (`{"type":"readProject","id":"..."}`)
/// so the wire stays forward-compatible when new variants land.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Capability {
    /// May call `spawn_agent` to create a subordinate agent.
    SpawnAgent,
    /// May call `send_to_agent` / `agent_lifecycle` / `delegate_task` on
    /// agents within scope.
    ControlAgent,
    /// May call `get_agent_state` on agents within scope.
    ReadAgent,
    /// May add / remove org members.
    ManageOrgMembers,
    /// May mutate billing plans / invoices.
    ManageBilling,
    /// May invoke long-lived processes (shells, background jobs).
    InvokeProcess,
    /// May post into the activity feed.
    PostToFeed,
    /// May call media-generation tools (image / video / audio).
    GenerateMedia,
    /// May read project `id`. Project ids are opaque strings owned by the host
    /// application (aura-os); the harness does not interpret them.
    #[serde(rename_all = "camelCase")]
    ReadProject {
        /// Opaque project identifier.
        id: String,
    },
    /// May write project `id`.
    #[serde(rename_all = "camelCase")]
    WriteProject {
        /// Opaque project identifier.
        id: String,
    },
}

/// The universe of orgs / projects / agents an agent may touch.
///
/// An empty scope on every axis means **universe** — i.e. no scope restriction,
/// the caller may operate across any org / project / agent. Non-empty lists
/// narrow the caller: each referenced value is explicitly whitelisted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentScope {
    /// Allowed org ids (empty = universe).
    #[serde(default)]
    pub orgs: Vec<String>,
    /// Allowed project ids (empty = universe).
    #[serde(default)]
    pub projects: Vec<String>,
    /// Allowed agent ids (empty = universe).
    #[serde(default)]
    pub agent_ids: Vec<String>,
}

impl AgentScope {
    /// True when no axis is restricted (universe scope).
    #[must_use]
    pub fn is_universe(&self) -> bool {
        self.orgs.is_empty() && self.projects.is_empty() && self.agent_ids.is_empty()
    }

    /// `self` contains `other` iff for each axis, either `self` is universe
    /// (empty) or every entry in `other` is present in `self`. An `other`
    /// universe can only be contained by a `self` universe on that axis.
    #[must_use]
    pub fn contains(&self, other: &Self) -> bool {
        axis_contains(&self.orgs, &other.orgs)
            && axis_contains(&self.projects, &other.projects)
            && axis_contains(&self.agent_ids, &other.agent_ids)
    }
}

fn axis_contains(parent: &[String], child: &[String]) -> bool {
    if parent.is_empty() {
        return true;
    }
    if child.is_empty() {
        return false;
    }
    child.iter().all(|c| parent.iter().any(|p| p == c))
}

/// A bundle of scope + capabilities attached to an agent record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPermissions {
    #[serde(default)]
    pub scope: AgentScope,
    #[serde(default)]
    pub capabilities: Vec<Capability>,
}

impl AgentPermissions {
    /// Fully permissive preset used for "CEO" agents in phase 6. Universe
    /// scope plus every variant of [`Capability`] (with a single illustrative
    /// project-scoped grant when applicable — no project-scoped variant is
    /// included here because project ids are host-specific).
    #[must_use]
    pub fn ceo_preset() -> Self {
        Self {
            scope: AgentScope::default(),
            capabilities: vec![
                Capability::SpawnAgent,
                Capability::ControlAgent,
                Capability::ReadAgent,
                Capability::ManageOrgMembers,
                Capability::ManageBilling,
                Capability::InvokeProcess,
                Capability::PostToFeed,
                Capability::GenerateMedia,
            ],
        }
    }

    /// Legacy default applied by the phase 6 migrator to existing
    /// `role == "super_agent"` records: identical to [`Self::ceo_preset`].
    #[must_use]
    pub fn legacy_default() -> Self {
        Self::ceo_preset()
    }

    /// Empty permissions: universe scope (vacuously), zero capabilities.
    /// Strict subset of every other `AgentPermissions`.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// True iff every capability in `other` is present in `self` **and**
    /// `other.scope` is contained in `self.scope`. Strict subset on the
    /// permission axis; scope follows [`AgentScope::contains`] semantics.
    #[must_use]
    pub fn contains(&self, other: &Self) -> bool {
        if !self.scope.contains(&other.scope) {
            return false;
        }
        other
            .capabilities
            .iter()
            .all(|c| self.capabilities.contains(c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope_with_org(org: &str) -> AgentScope {
        AgentScope {
            orgs: vec![org.to_string()],
            ..AgentScope::default()
        }
    }

    #[test]
    fn universe_scope_is_universe() {
        assert!(AgentScope::default().is_universe());
    }

    #[test]
    fn non_universe_scope_is_not_universe() {
        assert!(!scope_with_org("a").is_universe());
    }

    #[test]
    fn ceo_contains_empty() {
        assert!(AgentPermissions::ceo_preset().contains(&AgentPermissions::empty()));
    }

    #[test]
    fn empty_does_not_contain_ceo() {
        assert!(!AgentPermissions::empty().contains(&AgentPermissions::ceo_preset()));
    }

    #[test]
    fn narrower_scope_is_subset_of_wider() {
        let parent = AgentPermissions {
            scope: AgentScope {
                orgs: vec!["a".into(), "b".into()],
                ..AgentScope::default()
            },
            capabilities: vec![Capability::SpawnAgent, Capability::ControlAgent],
        };
        let child = AgentPermissions {
            scope: scope_with_org("a"),
            capabilities: vec![Capability::SpawnAgent],
        };
        assert!(parent.contains(&child));
    }

    #[test]
    fn disjoint_scopes_dont_contain_each_other() {
        let a = AgentPermissions {
            scope: scope_with_org("alpha"),
            ..AgentPermissions::default()
        };
        let b = AgentPermissions {
            scope: scope_with_org("beta"),
            ..AgentPermissions::default()
        };
        assert!(!a.contains(&b));
        assert!(!b.contains(&a));
    }

    #[test]
    fn universe_parent_contains_narrow_child() {
        let parent = AgentPermissions {
            scope: AgentScope::default(),
            capabilities: vec![Capability::SpawnAgent],
        };
        let child = AgentPermissions {
            scope: scope_with_org("only"),
            capabilities: vec![Capability::SpawnAgent],
        };
        assert!(parent.contains(&child));
    }

    #[test]
    fn narrow_parent_does_not_contain_universe_child() {
        let parent = AgentPermissions {
            scope: scope_with_org("only"),
            ..AgentPermissions::default()
        };
        let child = AgentPermissions {
            scope: AgentScope::default(),
            ..AgentPermissions::default()
        };
        assert!(!parent.contains(&child));
    }

    #[test]
    fn capability_escalation_is_denied() {
        let parent = AgentPermissions {
            scope: AgentScope::default(),
            capabilities: vec![Capability::SpawnAgent],
        };
        let child = AgentPermissions {
            scope: AgentScope::default(),
            capabilities: vec![Capability::SpawnAgent, Capability::ManageBilling],
        };
        assert!(!parent.contains(&child));
    }

    #[test]
    fn capability_serde_is_externally_tagged_camel_case() {
        let cap = Capability::ReadProject {
            id: "proj-1".into(),
        };
        let json = serde_json::to_value(&cap).unwrap();
        assert_eq!(json["type"], "readProject");
        assert_eq!(json["id"], "proj-1");
        let back: Capability = serde_json::from_value(json).unwrap();
        assert_eq!(cap, back);
    }

    #[test]
    fn agent_permissions_roundtrip() {
        let perms = AgentPermissions::ceo_preset();
        let json = serde_json::to_string(&perms).unwrap();
        let parsed: AgentPermissions = serde_json::from_str(&json).unwrap();
        assert_eq!(perms, parsed);
    }
}
