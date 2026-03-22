//! Context building for the kernel.

use aura_core::{hash, RecordEntry, Transaction};
use aura_reasoner::RecordSummary;
use tracing::debug;

/// Context for kernel processing.
#[derive(Debug, Clone)]
pub struct Context {
    /// Hash of the context inputs
    pub context_hash: [u8; 32],
    /// Record window summaries for the reasoner
    pub record_summaries: Vec<RecordSummary>,
}

/// Builder for kernel context.
pub struct ContextBuilder {
    tx_bytes: Vec<u8>,
    record_window: Vec<RecordEntry>,
}

impl ContextBuilder {
    /// Create a new context builder.
    #[must_use]
    pub fn new(tx: &Transaction) -> Self {
        let tx_bytes = serde_json::to_vec(tx).unwrap_or_default();
        Self {
            tx_bytes,
            record_window: Vec::new(),
        }
    }

    /// Add record window entries.
    #[must_use]
    pub fn with_record_window(mut self, entries: Vec<RecordEntry>) -> Self {
        self.record_window = entries;
        self
    }

    /// Build the context.
    #[must_use]
    pub fn build(self) -> Context {
        // Compute context hash
        let mut hasher = hash::Hasher::new();
        hasher.update(&self.tx_bytes);

        // Include minimal deterministic data from record window
        for entry in &self.record_window {
            hasher.update(&entry.seq.to_be_bytes());
            hasher.update(&entry.context_hash);
        }

        let context_hash = hasher.finalize();

        // Build record summaries for reasoner
        let record_summaries: Vec<RecordSummary> = self
            .record_window
            .iter()
            .map(|entry| {
                let action_kinds: Vec<_> = entry.actions.iter().map(|a| a.kind).collect();

                // Truncate payload for summary
                let payload_summary = if entry.tx.payload.len() > 200 {
                    Some(format!(
                        "{}...",
                        String::from_utf8_lossy(&entry.tx.payload[..200])
                    ))
                } else {
                    Some(String::from_utf8_lossy(&entry.tx.payload).to_string())
                };

                RecordSummary {
                    seq: entry.seq,
                    tx_kind: format!("{:?}", entry.tx.tx_type),
                    action_kinds,
                    payload_summary,
                }
            })
            .collect();

        debug!(
            hash = hex::encode(&context_hash[..8]),
            window_size = record_summaries.len(),
            "Context built"
        );

        Context {
            context_hash,
            record_summaries,
        }
    }
}

#[cfg(test)]
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

        let ctx1 = ContextBuilder::new(&tx).build();
        let ctx2 = ContextBuilder::new(&tx).build();

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

        let ctx1 = ContextBuilder::new(&tx).build();
        let ctx2 = ContextBuilder::new(&tx)
            .with_record_window(vec![entry])
            .build();

        assert_ne!(ctx1.context_hash, ctx2.context_hash);
    }

    #[test]
    fn test_context_hash_differs_with_different_tx() {
        let agent_id = AgentId::generate();
        let tx1 = Transaction::user_prompt(agent_id, "message 1");
        let tx2 = Transaction::user_prompt(agent_id, "message 2");

        let ctx1 = ContextBuilder::new(&tx1).build();
        let ctx2 = ContextBuilder::new(&tx2).build();

        assert_ne!(ctx1.context_hash, ctx2.context_hash);
    }

    #[test]
    fn test_context_hash_differs_with_window_order() {
        let agent_id = AgentId::generate();
        let tx = Transaction::user_prompt(agent_id, "test");

        let entry1 = create_test_entry(1, agent_id, TransactionType::UserPrompt, "first");
        let entry2 = create_test_entry(2, agent_id, TransactionType::UserPrompt, "second");

        let ctx_order1 = ContextBuilder::new(&tx)
            .with_record_window(vec![entry1.clone(), entry2.clone()])
            .build();

        let ctx_order2 = ContextBuilder::new(&tx)
            .with_record_window(vec![entry2, entry1])
            .build();

        // Order matters for context hash
        assert_ne!(ctx_order1.context_hash, ctx_order2.context_hash);
    }

    #[test]
    fn test_record_summaries_basic() {
        let agent_id = AgentId::generate();
        let tx = Transaction::user_prompt(agent_id, "current");

        let entry = create_test_entry(1, agent_id, TransactionType::UserPrompt, "hello world");

        let ctx = ContextBuilder::new(&tx)
            .with_record_window(vec![entry])
            .build();

        assert_eq!(ctx.record_summaries.len(), 1);
        assert_eq!(ctx.record_summaries[0].seq, 1);
        assert_eq!(ctx.record_summaries[0].tx_kind, "UserPrompt");
        assert!(ctx.record_summaries[0]
            .payload_summary
            .as_ref()
            .unwrap()
            .contains("hello world"));
    }

    #[test]
    fn test_record_summaries_with_actions() {
        let agent_id = AgentId::generate();
        let tx = Transaction::user_prompt(agent_id, "current");

        let entry =
            create_entry_with_actions(1, agent_id, &[ActionKind::Delegate, ActionKind::Reason]);

        let ctx = ContextBuilder::new(&tx)
            .with_record_window(vec![entry])
            .build();

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
            .with_record_window(vec![entry])
            .build();

        let summary = &ctx.record_summaries[0].payload_summary.as_ref().unwrap();
        assert!(summary.len() < 250); // Should be truncated
        assert!(summary.ends_with("..."));
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

        let ctx = ContextBuilder::new(&tx).with_record_window(entries).build();

        assert_eq!(ctx.record_summaries.len(), 4);
        assert_eq!(ctx.record_summaries[0].tx_kind, "UserPrompt");
        assert_eq!(ctx.record_summaries[1].tx_kind, "AgentMsg");
        assert_eq!(ctx.record_summaries[2].tx_kind, "SessionStart");
        assert_eq!(ctx.record_summaries[3].tx_kind, "UserPrompt");
    }

    #[test]
    fn test_context_empty_window() {
        let tx = Transaction::user_prompt(AgentId::generate(), "test");

        let ctx = ContextBuilder::new(&tx).with_record_window(vec![]).build();

        assert!(ctx.record_summaries.is_empty());
        // Context hash should still be valid
        assert_ne!(ctx.context_hash, [0u8; 32]);
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
            .with_record_window(vec![entry1])
            .build();

        let ctx2 = ContextBuilder::new(&tx)
            .with_record_window(vec![entry2])
            .build();

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

        let ctx = ContextBuilder::new(&tx).with_record_window(entries).build();

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

        let ctx = ContextBuilder::new(&tx).with_record_window(entries).build();

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
            .with_record_window(vec![entry])
            .build();

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
            .with_record_window(vec![entry])
            .build();

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
            .with_record_window(entries.clone())
            .build();
        let ctx2 = ContextBuilder::new(&tx)
            .with_record_window(entries)
            .build();

        assert_eq!(ctx1.context_hash, ctx2.context_hash);
        assert_eq!(ctx1.record_summaries.len(), ctx2.record_summaries.len());
    }
}
