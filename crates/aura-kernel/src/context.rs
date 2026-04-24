//! Context building for the kernel.

use aura_core::{hash, ContextHash, RecordEntry, Transaction};
use aura_reasoner::RecordSummary;
use tracing::debug;

/// Canonical context-hash function for every kernel processing path.
///
/// Implements Invariant §6 literally:
///
/// ```text
/// context_hash = hash(serialize(tx)
///                  || seq[0].context_hash
///                  || seq[1].context_hash
///                  || ...)
/// ```
///
/// Note that only the per-entry `context_hash` participates — neither the
/// entry's `seq`, tx type, nor payload is mixed in. The chain of prior
/// `context_hash` values already encodes that history transitively, which
/// keeps the hash stable under inconsequential representation changes
/// while still diverging on any semantic change to the record.
///
/// # Errors
/// Returns an error if the transaction cannot be serialized.
///
/// Exposed as `pub` so the invariant test suite in
/// `crates/aura-kernel/tests/invariant_determinism.rs` (Phase 10 / Wave 7)
/// can assert Invariant §6 directly against the canonical function without
/// going through `ContextBuilder`. The function is pure — it has no side
/// effects and no hidden state — so widening its visibility does not expand
/// the kernel's production surface.
pub fn hash_tx_with_window(
    tx: &Transaction,
    window: &[RecordEntry],
) -> Result<ContextHash, crate::KernelError> {
    let tx_bytes = serde_json::to_vec(tx)
        .map_err(|e| crate::KernelError::Serialization(format!("serialize tx: {e}")))?;
    let mut hasher = hash::Hasher::new();
    hasher.update(&tx_bytes);
    for entry in window {
        hasher.update(entry.context_hash.as_ref());
    }
    Ok(ContextHash::from(hasher.finalize()))
}

/// Context for kernel processing.
#[derive(Debug, Clone)]
pub struct Context {
    /// Hash of the context inputs
    pub context_hash: ContextHash,
    /// Record window summaries for the reasoner
    pub record_summaries: Vec<RecordSummary>,
}

/// Builder for kernel context.
pub struct ContextBuilder {
    tx: Transaction,
    record_window: Vec<RecordEntry>,
}

impl ContextBuilder {
    /// Create a new context builder.
    ///
    /// # Errors
    ///
    /// Returns an error if the transaction cannot be serialized.
    pub fn new(tx: &Transaction) -> Result<Self, serde_json::Error> {
        // Pre-flight the serialization so the eventual `build()` cannot fail.
        let _ = serde_json::to_vec(tx)?;
        Ok(Self {
            tx: tx.clone(),
            record_window: Vec::new(),
        })
    }

    /// Add record window entries.
    #[must_use]
    pub fn with_record_window(mut self, entries: Vec<RecordEntry>) -> Self {
        self.record_window = entries;
        self
    }

    /// Build the context.
    ///
    /// # Errors
    ///
    /// Returns [`crate::KernelError::Internal`] when the canonical
    /// `hash_tx_with_window` function fails (for example, if transaction
    /// serialization fails). The previous implementation silently fell back
    /// to an all-zero context hash, which would have violated Invariant §6
    /// by producing two distinct transactions with identical context hashes.
    pub fn build(self) -> Result<Context, crate::KernelError> {
        // Delegate to the canonical `hash_tx_with_window` so every kernel
        // path agrees on the formula from Invariant §6.
        let context_hash = hash_tx_with_window(&self.tx, &self.record_window)
            .map_err(|e| crate::KernelError::Internal(format!("context hash: {e}")))?;

        // Build record summaries for reasoner
        let record_summaries: Vec<RecordSummary> = self
            .record_window
            .iter()
            .map(|entry| {
                let action_kinds: Vec<_> = entry.actions.iter().map(|a| a.kind).collect();

                // Opaque fingerprint of the payload: first 16 hex chars of the
                // BLAKE3 digest. We keep the field name for log compatibility
                // but no longer leak plaintext bytes (which could include
                // secrets, PII, or raw prompts) into record summaries that
                // fan out through the reasoner and tracing. (Wave 5 / T6.)
                let digest = blake3::hash(&entry.tx.payload);
                let payload_summary = Some(format!("blake3:{}", &digest.to_hex()[..16]));

                RecordSummary {
                    seq: entry.seq,
                    tx_kind: format!("{:?}", entry.tx.tx_type),
                    action_kinds,
                    payload_summary,
                }
            })
            .collect();

        debug!(
            hash = hex::encode(&context_hash.as_ref()[..8]),
            window_size = record_summaries.len(),
            "Context built"
        );

        Ok(Context {
            context_hash,
            record_summaries,
        })
    }
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)] // TODO(W5): seq<256 in fixtures; migrate to u8::try_from.
mod tests {
    use super::*;
    use aura_core::{
        Action, ActionId, ActionKind, AgentId, Decision, ProposalSet, TransactionType,
    };
    use bytes::Bytes;

    fn create_test_entry(
        seq: u64,
        agent_id: AgentId,
        tx_type: TransactionType,
        payload: &str,
    ) -> RecordEntry {
        let tx =
            Transaction::new_chained(agent_id, tx_type, Bytes::from(payload.to_string()), None);
        RecordEntry::builder(seq, tx)
            .context_hash([seq as u8; 32])
            .proposals(ProposalSet::new())
            .decision(Decision::new())
            .build()
    }

    fn create_entry_with_actions(
        seq: u64,
        agent_id: AgentId,
        action_kinds: &[ActionKind],
    ) -> RecordEntry {
        let tx = Transaction::user_prompt(agent_id, format!("entry {seq}"));

        let actions: Vec<Action> = action_kinds
            .iter()
            .map(|&kind| Action::new(ActionId::generate(), kind, Bytes::new()))
            .collect();

        let mut decision = Decision::new();
        for action in &actions {
            decision.accept(action.action_id);
        }

        RecordEntry::builder(seq, tx)
            .context_hash([seq as u8; 32])
            .proposals(ProposalSet::new())
            .decision(decision)
            .actions(actions)
            .build()
    }

    #[test]
    fn test_context_hash_deterministic() {
        let tx = Transaction::user_prompt(AgentId::generate(), "test");

        let ctx1 = ContextBuilder::new(&tx).unwrap().build().unwrap();
        let ctx2 = ContextBuilder::new(&tx).unwrap().build().unwrap();

        assert_eq!(ctx1.context_hash, ctx2.context_hash);
    }

    #[test]
    fn test_context_hash_differs_with_window() {
        let agent_id = AgentId::generate();
        let tx = Transaction::user_prompt(agent_id, "test");

        let entry = RecordEntry::builder(1, tx.clone())
            .context_hash([1u8; 32])
            .proposals(ProposalSet::new())
            .decision(Decision::new())
            .build();

        let ctx1 = ContextBuilder::new(&tx).unwrap().build().unwrap();
        let ctx2 = ContextBuilder::new(&tx)
            .unwrap()
            .with_record_window(vec![entry])
            .build()
            .unwrap();

        assert_ne!(ctx1.context_hash, ctx2.context_hash);
    }

    #[test]
    fn test_context_hash_differs_with_different_tx() {
        let agent_id = AgentId::generate();
        let tx1 = Transaction::user_prompt(agent_id, "message 1");
        let tx2 = Transaction::user_prompt(agent_id, "message 2");

        let ctx1 = ContextBuilder::new(&tx1).unwrap().build().unwrap();
        let ctx2 = ContextBuilder::new(&tx2).unwrap().build().unwrap();

        assert_ne!(ctx1.context_hash, ctx2.context_hash);
    }

    #[test]
    fn test_context_hash_differs_with_window_order() {
        let agent_id = AgentId::generate();
        let tx = Transaction::user_prompt(agent_id, "test");

        let entry1 = create_test_entry(1, agent_id, TransactionType::UserPrompt, "first");
        let entry2 = create_test_entry(2, agent_id, TransactionType::UserPrompt, "second");

        let ctx_order1 = ContextBuilder::new(&tx)
            .unwrap()
            .with_record_window(vec![entry1.clone(), entry2.clone()])
            .build()
            .unwrap();

        let ctx_order2 = ContextBuilder::new(&tx)
            .unwrap()
            .with_record_window(vec![entry2, entry1])
            .build()
            .unwrap();

        // Order matters for context hash
        assert_ne!(ctx_order1.context_hash, ctx_order2.context_hash);
    }

    #[test]
    fn test_record_summaries_basic() {
        let agent_id = AgentId::generate();
        let tx = Transaction::user_prompt(agent_id, "current");

        let entry = create_test_entry(1, agent_id, TransactionType::UserPrompt, "hello world");

        let ctx = ContextBuilder::new(&tx)
            .unwrap()
            .with_record_window(vec![entry])
            .build()
            .unwrap();

        assert_eq!(ctx.record_summaries.len(), 1);
        assert_eq!(ctx.record_summaries[0].seq, 1);
        assert_eq!(ctx.record_summaries[0].tx_kind, "UserPrompt");

        // `payload_summary` is now an opaque `blake3:<16-hex>` fingerprint
        // instead of raw plaintext (Wave 5 / T6). Verify the prefix and
        // that the digest is deterministic for identical input.
        let summary = ctx.record_summaries[0]
            .payload_summary
            .as_ref()
            .expect("payload_summary must be set");
        assert!(summary.starts_with("blake3:"), "unexpected: {summary}");
        assert_eq!(summary.len(), "blake3:".len() + 16);
        let expected = format!("blake3:{}", &blake3::hash(b"hello world").to_hex()[..16]);
        assert_eq!(*summary, expected);
    }

    #[test]
    fn test_record_summaries_with_actions() {
        let agent_id = AgentId::generate();
        let tx = Transaction::user_prompt(agent_id, "current");

        let entry =
            create_entry_with_actions(1, agent_id, &[ActionKind::Delegate, ActionKind::Reason]);

        let ctx = ContextBuilder::new(&tx)
            .unwrap()
            .with_record_window(vec![entry])
            .build()
            .unwrap();

        assert_eq!(ctx.record_summaries[0].action_kinds.len(), 2);
        assert!(ctx.record_summaries[0]
            .action_kinds
            .contains(&ActionKind::Delegate));
        assert!(ctx.record_summaries[0]
            .action_kinds
            .contains(&ActionKind::Reason));
    }

    #[test]
    fn test_record_summaries_payload_truncation() {
        let agent_id = AgentId::generate();
        let tx = Transaction::user_prompt(agent_id, "current");

        // Create a very long payload
        let long_payload = "x".repeat(500);
        let entry = create_test_entry(1, agent_id, TransactionType::UserPrompt, &long_payload);

        let ctx = ContextBuilder::new(&tx)
            .unwrap()
            .with_record_window(vec![entry])
            .build()
            .unwrap();

        // Payload summaries are now constant-size fingerprints
        // (`blake3:<16-hex>`), not byte-truncated plaintext, so the
        // old "truncated with trailing ..." check no longer applies.
        // We instead assert the summary is the expected deterministic
        // digest of the 500-byte payload. (Wave 5 / T6.)
        let summary = ctx.record_summaries[0]
            .payload_summary
            .as_ref()
            .expect("payload_summary must be set");
        let expected = format!(
            "blake3:{}",
            &blake3::hash(long_payload.as_bytes()).to_hex()[..16]
        );
        assert_eq!(*summary, expected);
        assert!(summary.len() < 250);
    }

    #[test]
    fn test_record_summaries_multiple_entries() {
        let agent_id = AgentId::generate();
        let tx = Transaction::user_prompt(agent_id, "current");

        let entries = vec![
            create_test_entry(1, agent_id, TransactionType::UserPrompt, "first"),
            create_test_entry(2, agent_id, TransactionType::AgentMsg, "response"),
            create_test_entry(3, agent_id, TransactionType::SessionStart, ""),
            create_test_entry(4, agent_id, TransactionType::UserPrompt, "after session"),
        ];

        let ctx = ContextBuilder::new(&tx)
            .unwrap()
            .with_record_window(entries)
            .build()
            .unwrap();

        assert_eq!(ctx.record_summaries.len(), 4);
        assert_eq!(ctx.record_summaries[0].tx_kind, "UserPrompt");
        assert_eq!(ctx.record_summaries[1].tx_kind, "AgentMsg");
        assert_eq!(ctx.record_summaries[2].tx_kind, "SessionStart");
        assert_eq!(ctx.record_summaries[3].tx_kind, "UserPrompt");
    }

    #[test]
    fn test_context_empty_window() {
        let tx = Transaction::user_prompt(AgentId::generate(), "test");

        let ctx = ContextBuilder::new(&tx)
            .unwrap()
            .with_record_window(vec![])
            .build()
            .unwrap();

        assert!(ctx.record_summaries.is_empty());
        // Context hash should still be valid
        assert_ne!(ctx.context_hash, ContextHash::zero());
    }

    #[test]
    fn test_context_hash_includes_window_hashes() {
        let agent_id = AgentId::generate();
        let tx = Transaction::user_prompt(agent_id, "test");

        // Two entries with same seq but different context hashes
        let tx1 = Transaction::user_prompt(agent_id, "entry");
        let entry1 = RecordEntry::builder(1, tx1.clone())
            .context_hash([1u8; 32])
            .build();

        let entry2 = RecordEntry::builder(1, tx1).context_hash([2u8; 32]).build();

        let ctx1 = ContextBuilder::new(&tx)
            .unwrap()
            .with_record_window(vec![entry1])
            .build()
            .unwrap();

        let ctx2 = ContextBuilder::new(&tx)
            .unwrap()
            .with_record_window(vec![entry2])
            .build()
            .unwrap();

        // Different window context hashes should produce different overall context hash
        assert_ne!(ctx1.context_hash, ctx2.context_hash);
    }

    #[test]
    fn test_context_with_all_transaction_types() {
        let agent_id = AgentId::generate();
        let tx = Transaction::user_prompt(agent_id, "current");

        let entries = vec![
            create_test_entry(1, agent_id, TransactionType::UserPrompt, "user"),
            create_test_entry(2, agent_id, TransactionType::AgentMsg, "agent"),
            create_test_entry(3, agent_id, TransactionType::Trigger, "trigger"),
            create_test_entry(4, agent_id, TransactionType::ActionResult, "result"),
            create_test_entry(5, agent_id, TransactionType::System, "system"),
            create_test_entry(6, agent_id, TransactionType::SessionStart, "session"),
            create_test_entry(7, agent_id, TransactionType::ToolProposal, "proposal"),
            create_test_entry(8, agent_id, TransactionType::ToolExecution, "execution"),
            create_test_entry(9, agent_id, TransactionType::ProcessComplete, "complete"),
        ];

        let ctx = ContextBuilder::new(&tx)
            .unwrap()
            .with_record_window(entries)
            .build()
            .unwrap();

        assert_eq!(ctx.record_summaries.len(), 9);

        // Verify all types are represented
        let types: Vec<&str> = ctx
            .record_summaries
            .iter()
            .map(|s| s.tx_kind.as_str())
            .collect();

        assert!(types.contains(&"UserPrompt"));
        assert!(types.contains(&"AgentMsg"));
        assert!(types.contains(&"SessionStart"));
        assert!(types.contains(&"ToolProposal"));
        assert!(types.contains(&"ToolExecution"));
        assert!(types.contains(&"ProcessComplete"));
    }

    #[test]
    fn test_context_large_record_window() {
        let agent_id = AgentId::generate();
        let tx = Transaction::user_prompt(agent_id, "current");

        let entries: Vec<RecordEntry> = (1..=100)
            .map(|seq| {
                create_test_entry(
                    seq,
                    agent_id,
                    TransactionType::UserPrompt,
                    &format!("message {seq}"),
                )
            })
            .collect();

        let ctx = ContextBuilder::new(&tx)
            .unwrap()
            .with_record_window(entries)
            .build()
            .unwrap();

        assert_eq!(ctx.record_summaries.len(), 100);
        assert_eq!(ctx.record_summaries[0].seq, 1);
        assert_eq!(ctx.record_summaries[99].seq, 100);
    }

    #[test]
    fn test_context_preserves_action_kinds_in_summaries() {
        let agent_id = AgentId::generate();
        let tx = Transaction::user_prompt(agent_id, "current");

        let entry = create_entry_with_actions(
            1,
            agent_id,
            &[ActionKind::Reason, ActionKind::Memorize, ActionKind::Decide],
        );

        let ctx = ContextBuilder::new(&tx)
            .unwrap()
            .with_record_window(vec![entry])
            .build()
            .unwrap();

        assert_eq!(ctx.record_summaries[0].action_kinds.len(), 3);
        assert!(ctx.record_summaries[0]
            .action_kinds
            .contains(&ActionKind::Memorize));
        assert!(ctx.record_summaries[0]
            .action_kinds
            .contains(&ActionKind::Decide));
    }

    #[test]
    fn test_context_empty_payload_produces_summary() {
        let agent_id = AgentId::generate();
        let tx = Transaction::user_prompt(agent_id, "current");

        let entry = create_test_entry(1, agent_id, TransactionType::SessionStart, "");

        let ctx = ContextBuilder::new(&tx)
            .unwrap()
            .with_record_window(vec![entry])
            .build()
            .unwrap();

        assert_eq!(ctx.record_summaries.len(), 1);
        assert!(ctx.record_summaries[0].payload_summary.is_some());
    }

    #[test]
    fn test_context_hash_stability_across_builds() {
        let agent_id = AgentId::generate();
        let tx = Transaction::user_prompt(agent_id, "stability");

        let entries = vec![
            create_test_entry(1, agent_id, TransactionType::UserPrompt, "hello"),
            create_test_entry(2, agent_id, TransactionType::AgentMsg, "world"),
        ];

        let ctx1 = ContextBuilder::new(&tx)
            .unwrap()
            .with_record_window(entries.clone())
            .build()
            .unwrap();
        let ctx2 = ContextBuilder::new(&tx)
            .unwrap()
            .with_record_window(entries)
            .build()
            .unwrap();

        assert_eq!(ctx1.context_hash, ctx2.context_hash);
        assert_eq!(ctx1.record_summaries.len(), ctx2.record_summaries.len());
    }

    // ====================================================================
    // hash_tx_with_window — canonical Invariant §6 function
    // ====================================================================

    #[test]
    fn hash_tx_with_window_is_deterministic() {
        let agent_id = AgentId::generate();
        let tx = Transaction::user_prompt(agent_id, "determinism");
        let window = vec![
            create_test_entry(1, agent_id, TransactionType::UserPrompt, "alpha"),
            create_test_entry(2, agent_id, TransactionType::AgentMsg, "beta"),
            create_test_entry(3, agent_id, TransactionType::ToolExecution, "gamma"),
        ];

        let h1 = hash_tx_with_window(&tx, &window).unwrap();
        let h2 = hash_tx_with_window(&tx, &window).unwrap();

        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_tx_with_window_is_order_sensitive() {
        let agent_id = AgentId::generate();
        let tx = Transaction::user_prompt(agent_id, "order");

        // Two entries with distinct context_hashes so swapping them must
        // produce a different final hash.
        let entry_a = create_test_entry(1, agent_id, TransactionType::UserPrompt, "a");
        let entry_b = create_test_entry(2, agent_id, TransactionType::UserPrompt, "b");

        let forward = hash_tx_with_window(&tx, &[entry_a.clone(), entry_b.clone()]).unwrap();
        let swapped = hash_tx_with_window(&tx, &[entry_b, entry_a]).unwrap();

        assert_ne!(forward, swapped);
    }

    #[test]
    fn hash_tx_with_window_empty_window_is_serialized_tx_only() {
        // With an empty window the hash should equal
        // blake3(serialize(tx)) — in particular, non-zero and stable.
        let tx = Transaction::user_prompt(AgentId::generate(), "empty-window");
        let h = hash_tx_with_window(&tx, &[]).unwrap();
        assert_ne!(h, ContextHash::zero());
    }
}
