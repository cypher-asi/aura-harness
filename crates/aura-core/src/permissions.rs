//! Agent permission primitives — the single source of truth for what an
//! agent can do.
//!
//! # Core model
//!
//! A single `Agent` type exists across the system. Its `role` field is a
//! free-text display label (e.g. `"CEO"`, `"Developer"`) with **no system
//! meaning**. What an agent can actually do is determined **entirely** by
//! its [`AgentPermissions`]:
//!
//! - [`AgentPermissions::capabilities`] — the set of [`Capability`] grants
//!   controlling which operations (spawn, control, manage billing, etc.)
//!   the agent may perform.
//! - [`AgentPermissions::scope`] — an [`AgentScope`] narrowing which orgs,
//!   projects, and agents the caller may touch.
//! - [`AgentPermissions::ceo_preset`] grants every capability plus universe
//!   scope (the bootstrap super-agent).
//! - [`AgentPermissions::empty`] grants nothing (regular agents).
//! - Spawned children must receive a **strict subset** of their parent's
//!   permissions — enforced via [`AgentPermissions::contains`].
//!
//! Enforcement is unconditional. There is no Cargo feature toggle, no
//! `Option<AgentPermissions>` anywhere in persisted state, and no
//! role-based fallback. Every agent record carries a required
//! `AgentPermissions` value; every session opens with a required
//! permissions bundle on `SessionInit`; every `Delegate` proposal runs
//! through the policy gate.

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
    /// May call `list_agents` to discover agents within scope.
    ListAgents,
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
    /// Wildcard read access over every project in the bundle's scope.
    /// Satisfies any `ReadProject { id }` requirement without enumerating
    /// ids. Held by the CEO preset so the unified tool-surface filter
    /// can drop the legacy `is_ceo_preset()` short-circuit; regular
    /// bundles can also carry it when the caller genuinely has
    /// org-wide read access.
    ReadAllProjects,
    /// Wildcard write access over every project in the bundle's scope.
    /// Strict superset of [`Capability::ReadAllProjects`]: satisfies any
    /// `WriteProject { id }` requirement and (by write-implies-read)
    /// any `ReadProject { id }` requirement too.
    WriteAllProjects,
}

impl Capability {
    /// True iff `self` satisfies the project-scoped requirement `required`.
    ///
    /// Wildcard lifting rules (mirrored from the authoritative
    /// `aura-os-agent-tools` `permissions_satisfy_requirements`):
    ///
    /// * `ReadProject { id }` is satisfied by any of:
    ///   - `ReadProject { id }` (exact match),
    ///   - `WriteProject { id }` (write implies read),
    ///   - [`Capability::ReadAllProjects`] (wildcard),
    ///   - [`Capability::WriteAllProjects`] (wildcard write implies
    ///     wildcard read).
    /// * `WriteProject { id }` is satisfied by any of:
    ///   - `WriteProject { id }` (exact match),
    ///   - [`Capability::WriteAllProjects`] (wildcard).
    /// * For any other `required` the rule degenerates to exact equality.
    ///
    /// This is the single helper every enforcement site in the harness
    /// should route through to stay consistent with the server-side
    /// policy.
    #[must_use]
    pub fn satisfies(&self, required: &Capability) -> bool {
        match (self, required) {
            // Exact match always works.
            (held, req) if held == req => true,
            // Project wildcards lift to project-scoped requirements.
            (Capability::ReadAllProjects, Capability::ReadProject { .. }) => true,
            (Capability::WriteAllProjects, Capability::ReadProject { .. }) => true,
            (Capability::WriteAllProjects, Capability::WriteProject { .. }) => true,
            // Write implies read for the same project id.
            (Capability::WriteProject { id: held_id }, Capability::ReadProject { id: req_id }) => {
                held_id == req_id
            }
            _ => false,
        }
    }
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
                Capability::ListAgents,
                Capability::ManageOrgMembers,
                Capability::ManageBilling,
                Capability::InvokeProcess,
                Capability::PostToFeed,
                Capability::GenerateMedia,
                // Wildcard project caps replace the legacy
                // `is_ceo_preset()` short-circuit: with these in hand
                // the CEO satisfies any `ReadProject { id }` /
                // `WriteProject { id }` requirement through the normal
                // `Capability::satisfies` path, like any other bundle
                // that happens to carry a wildcard.
                Capability::ReadAllProjects,
                Capability::WriteAllProjects,
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

    /// True iff every capability in `other` is satisfied by `self` **and**
    /// `other.scope` is contained in `self.scope`. Scope follows
    /// [`AgentScope::contains`] semantics; capability containment uses
    /// [`Capability::satisfies`] so wildcard project caps on the parent
    /// cover exact-id project caps on the child. Without this lifting,
    /// a CEO parent (holding [`Capability::WriteAllProjects`]) couldn't
    /// spawn a child asking for `WriteProject { id: "x" }`, even though
    /// the parent's bundle is strictly more permissive.
    #[must_use]
    pub fn contains(&self, other: &Self) -> bool {
        if !self.scope.contains(&other.scope) {
            return false;
        }
        other
            .capabilities
            .iter()
            .all(|req| self.capabilities.iter().any(|held| held.satisfies(req)))
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
    fn write_project_satisfies_read_project_for_same_id() {
        let held = Capability::WriteProject {
            id: "proj-1".into(),
        };
        let req = Capability::ReadProject {
            id: "proj-1".into(),
        };
        assert!(held.satisfies(&req));
    }

    #[test]
    fn read_all_projects_satisfies_read_project() {
        let req = Capability::ReadProject { id: "any".into() };
        assert!(Capability::ReadAllProjects.satisfies(&req));
    }

    #[test]
    fn read_all_projects_does_not_satisfy_write_project() {
        let req = Capability::WriteProject { id: "p".into() };
        assert!(!Capability::ReadAllProjects.satisfies(&req));
    }

    #[test]
    fn write_all_projects_satisfies_both_read_and_write_project() {
        let r = Capability::ReadProject { id: "p".into() };
        let w = Capability::WriteProject { id: "p".into() };
        assert!(Capability::WriteAllProjects.satisfies(&r));
        assert!(Capability::WriteAllProjects.satisfies(&w));
    }

    #[test]
    fn ceo_preset_contains_project_scoped_child() {
        let parent = AgentPermissions::ceo_preset();
        let child = AgentPermissions {
            scope: AgentScope::default(),
            capabilities: vec![
                Capability::ReadProject { id: "p".into() },
                Capability::WriteProject { id: "q".into() },
            ],
        };
        assert!(parent.contains(&child));
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
