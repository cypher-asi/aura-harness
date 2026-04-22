//! Tool permission levels.
//!
//! Pure data type describing how the policy engine should treat a
//! particular `(agent, tool)` pair. Lives in `aura-core` rather than
//! `aura-kernel` so crates on the "outside" of the kernel — notably the
//! `DomainApi` in `aura-tools` used to fetch per-agent overrides from
//! aura-network — can marshal these values without pulling in the
//! kernel itself.
//!
//! `RequireApproval` was previously called `AlwaysAsk`. The rename
//! (security audit Phase 6) clarified the semantics: the kernel denies
//! unless [`aura_kernel::Kernel::grant_approval`] has registered an
//! explicit single-use approval for the `(agent_id, tool, args_hash)`
//! triple. The old serde tag is preserved via `#[serde(alias)]`.

use serde::{Deserialize, Serialize};

/// Permission level for tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionLevel {
    /// Always allowed without asking.
    AlwaysAllow,
    /// Ask once per session, then remember.
    AskOnce,
    /// Deny unless the caller has registered an explicit single-use
    /// approval for the exact `(agent_id, tool, args_hash)` triple.
    /// Consumed on first match.
    #[serde(alias = "always_ask")]
    RequireApproval,
    /// Never allowed.
    Deny,
}
