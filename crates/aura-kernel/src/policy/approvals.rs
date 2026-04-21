//! Single-use tool-call approvals.
//!
//! The policy engine may classify a proposal as
//! [`PolicyVerdict::RequireApproval`](super::PolicyVerdict::RequireApproval).
//! The kernel never pauses for an interactive prompt; instead, an
//! out-of-band authenticated caller (typically via
//! `POST /tool-approval`) registers an explicit, single-use approval
//! for the exact `(agent_id, tool, args_hash)` triple. The next time
//! that proposal arrives, the kernel **consumes** the approval and
//! allows the call through. A second attempt with the same hash must
//! request a fresh grant.
//!
//! The registry is deliberately shared across the short-lived per-agent
//! [`crate::Kernel`] instances built on demand by the scheduler, so a
//! grant survives the kernel that was active when the proposal was
//! first denied.

use aura_core::AgentId;
use std::collections::HashSet;
use std::sync::{Arc, RwLock};

/// Key identifying a single pre-approved tool invocation.
///
/// The `args_hash` is a Blake3-32 digest of the canonical JSON encoding
/// of the tool's `args` (`serde_json`'s default `BTreeMap` serializer
/// yields lexicographically sorted keys, so the digest is stable across
/// runs and platforms). Two invocations with identical args hash to the
/// same key, which matches how an operator would think about approving
/// "this exact call" vs "any call of this tool".
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ApprovalKey {
    /// Agent the approval was issued to. Approvals do not leak across
    /// agents even when the underlying tool + args are identical.
    pub agent_id: AgentId,
    /// Tool name the approval targets, e.g. `"run_command"`.
    pub tool: String,
    /// Blake3-32 digest of the canonical JSON args.
    pub args_hash: [u8; 32],
}

impl ApprovalKey {
    /// Construct an `ApprovalKey`.
    #[must_use]
    pub fn new(agent_id: AgentId, tool: impl Into<String>, args_hash: [u8; 32]) -> Self {
        Self {
            agent_id,
            tool: tool.into(),
            args_hash,
        }
    }

    /// Compute the canonical Blake3 args hash used by the kernel and
    /// router when building `ApprovalKey`s from a raw `args` JSON value.
    ///
    /// `serde_json::to_vec` on a `serde_json::Value` sorts object keys
    /// because `Value::Object` is backed by `BTreeMap` in the default
    /// (non-`preserve_order`) build. That gives us a canonical byte
    /// sequence without an explicit canonicalization pass.
    #[must_use]
    pub fn hash_args(args: &serde_json::Value) -> [u8; 32] {
        let encoded = serde_json::to_vec(args).unwrap_or_default();
        *blake3::hash(&encoded).as_bytes()
    }
}

/// Shared, interior-mutable store of pending single-use approvals.
///
/// Cheap to `clone()` — backed by an `Arc<RwLock<_>>` so the scheduler
/// can hand a handle to each short-lived per-agent [`crate::Kernel`]
/// and the HTTP layer in one call.
#[derive(Debug, Default, Clone)]
pub struct ApprovalRegistry {
    inner: Arc<RwLock<HashSet<ApprovalKey>>>,
}

impl ApprovalRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a one-shot approval for `(agent_id, tool, args_hash)`.
    ///
    /// Idempotent: granting the same triple twice leaves a single
    /// pending approval. A subsequent `take` removes it.
    pub fn grant(&self, agent_id: AgentId, tool: &str, args_hash: [u8; 32]) {
        let key = ApprovalKey::new(agent_id, tool, args_hash);
        self.inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key);
    }

    /// Remove a pending approval without consuming it via a tool call.
    ///
    /// Returns `true` when an entry was actually removed, matching the
    /// `HashSet::remove` contract. Operators use this to back out an
    /// accidental grant before the agent re-proposes.
    pub fn revoke(&self, agent_id: AgentId, tool: &str, args_hash: [u8; 32]) -> bool {
        let key = ApprovalKey::new(agent_id, tool, args_hash);
        self.inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&key)
    }

    /// Consume a pending approval if one matches the triple.
    ///
    /// Returns `true` when a match was found and removed. The kernel's
    /// `process_tool_proposal` path calls this exactly once per
    /// proposal that the policy classified as
    /// [`super::PolicyVerdict::RequireApproval`].
    #[must_use]
    pub fn take(&self, agent_id: AgentId, tool: &str, args_hash: [u8; 32]) -> bool {
        let key = ApprovalKey::new(agent_id, tool, args_hash);
        self.inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&key)
    }

    /// Non-consuming check. Primarily used by tests.
    #[must_use]
    pub fn contains(&self, agent_id: AgentId, tool: &str, args_hash: [u8; 32]) -> bool {
        let key = ApprovalKey::new(agent_id, tool, args_hash);
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_args_is_canonical_across_key_order() {
        // `serde_json::Value::Object` is BTreeMap-backed in the default
        // build, so these two values serialize identically.
        let a = serde_json::json!({"b": 1, "a": 2});
        let b = serde_json::json!({"a": 2, "b": 1});
        assert_eq!(ApprovalKey::hash_args(&a), ApprovalKey::hash_args(&b));
    }

    #[test]
    fn grant_then_take_consumes() {
        let reg = ApprovalRegistry::new();
        let agent = AgentId::generate();
        let h = ApprovalKey::hash_args(&serde_json::json!({"x": 1}));
        reg.grant(agent, "run_command", h);
        assert!(reg.contains(agent, "run_command", h));
        assert!(reg.take(agent, "run_command", h));
        assert!(!reg.contains(agent, "run_command", h));
        // Second take on a consumed approval fails.
        assert!(!reg.take(agent, "run_command", h));
    }

    #[test]
    fn revoke_returns_true_only_when_present() {
        let reg = ApprovalRegistry::new();
        let agent = AgentId::generate();
        let h = ApprovalKey::hash_args(&serde_json::json!({}));
        assert!(!reg.revoke(agent, "run_command", h));
        reg.grant(agent, "run_command", h);
        assert!(reg.revoke(agent, "run_command", h));
        assert!(!reg.revoke(agent, "run_command", h));
    }

    #[test]
    fn different_agents_isolated() {
        let reg = ApprovalRegistry::new();
        let a1 = AgentId::generate();
        let a2 = AgentId::generate();
        let h = ApprovalKey::hash_args(&serde_json::json!({}));
        reg.grant(a1, "run_command", h);
        assert!(!reg.contains(a2, "run_command", h));
    }
}
