//! Property-based tests using proptest.
//!
//! These tests verify invariants using randomly generated inputs.

use proptest::prelude::*;

// ============================================================================
// ID Roundtrip Tests
// ============================================================================

#[allow(deprecated)]
mod id_tests {
    use super::*;
    use aura_core::{ActionId, AgentId, TxId};

    proptest! {
        /// AgentId roundtrips through hex encoding.
        #[test]
        fn agent_id_hex_roundtrip(bytes: [u8; 32]) {
            let id = AgentId::new(bytes);
            let hex = id.to_hex();
            let parsed = AgentId::from_hex(&hex).unwrap();
            prop_assert_eq!(id, parsed);
        }

        /// AgentId roundtrips through JSON serialization.
        #[test]
        fn agent_id_json_roundtrip(bytes: [u8; 32]) {
            let id = AgentId::new(bytes);
            let json = serde_json::to_string(&id).unwrap();
            let parsed: AgentId = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(id, parsed);
        }

        /// TxId from same content produces same ID.
        #[test]
        fn tx_id_deterministic(content: Vec<u8>) {
            let id1 = TxId::from_content(&content);
            let id2 = TxId::from_content(&content);
            prop_assert_eq!(id1, id2);
        }

        /// TxId from different content produces different IDs (with high probability).
        #[test]
        fn tx_id_different_content(content1: Vec<u8>, content2: Vec<u8>) {
            prop_assume!(content1 != content2);
            let id1 = TxId::from_content(&content1);
            let id2 = TxId::from_content(&content2);
            prop_assert_ne!(id1, id2);
        }

        /// ActionId roundtrips through hex encoding.
        #[test]
        fn action_id_hex_roundtrip(bytes: [u8; 16]) {
            let id = ActionId::new(bytes);
            let hex = id.to_hex();
            let parsed = ActionId::from_hex(&hex).unwrap();
            prop_assert_eq!(id, parsed);
        }
    }
}

// ============================================================================
// Hash Tests
// ============================================================================

mod hash_tests {
    use super::*;
    use aura_core::hash;

    proptest! {
        /// Hashing is deterministic.
        #[test]
        fn hash_deterministic(data: Vec<u8>) {
            let hash1 = hash::hash_bytes(&data);
            let hash2 = hash::hash_bytes(&data);
            prop_assert_eq!(hash1, hash2);
        }

        /// Different data produces different hashes (with high probability).
        #[test]
        fn hash_different_data(data1: Vec<u8>, data2: Vec<u8>) {
            prop_assume!(data1 != data2);
            let hash1 = hash::hash_bytes(&data1);
            let hash2 = hash::hash_bytes(&data2);
            prop_assert_ne!(hash1, hash2);
        }

        /// Hash order matters for hash_many.
        #[test]
        fn hash_many_order_matters(part1: Vec<u8>, part2: Vec<u8>) {
            // Both parts must be non-empty and different for order to matter
            prop_assume!(part1 != part2 && !part1.is_empty() && !part2.is_empty());
            let hash1 = hash::hash_many(&[&part1, &part2]);
            let hash2 = hash::hash_many(&[&part2, &part1]);
            prop_assert_ne!(hash1, hash2);
        }

        /// Incremental hasher produces same result as hash_many.
        #[test]
        fn incremental_hasher_equivalent(parts: Vec<Vec<u8>>) {
            let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
            let direct = hash::hash_many(&refs);

            let mut hasher = hash::Hasher::new();
            for part in &parts {
                hasher.update(part);
            }
            let incremental = hasher.finalize();

            prop_assert_eq!(direct, incremental);
        }
    }
}

// ============================================================================
// Key Encoding Tests
// ============================================================================

mod key_tests {
    use super::*;
    use aura_core::AgentId;
    use aura_store::{AgentMetaKey, InboxKey, KeyCodec, MetaField, RecordKey};

    proptest! {
        /// RecordKey roundtrips through encoding.
        #[test]
        fn record_key_roundtrip(agent_bytes: [u8; 32], seq: u64) {
            let agent_id = AgentId::new(agent_bytes);
            let key = RecordKey::new(agent_id, seq);
            let encoded = key.encode();
            let decoded = RecordKey::decode(&encoded).unwrap();
            prop_assert_eq!(key, decoded);
        }

        /// RecordKey encoding preserves ordering.
        #[test]
        fn record_key_ordering(agent_bytes: [u8; 32], seq1: u64, seq2: u64) {
            let agent_id = AgentId::new(agent_bytes);
            let key1 = RecordKey::new(agent_id, seq1).encode();
            let key2 = RecordKey::new(agent_id, seq2).encode();

            if seq1 < seq2 {
                prop_assert!(key1 < key2);
            } else if seq1 > seq2 {
                prop_assert!(key1 > key2);
            } else {
                prop_assert_eq!(key1, key2);
            }
        }

        /// InboxKey roundtrips through encoding.
        #[test]
        fn inbox_key_roundtrip(agent_bytes: [u8; 32], seq: u64) {
            let agent_id = AgentId::new(agent_bytes);
            let key = InboxKey::new(agent_id, seq);
            let encoded = key.encode();
            let decoded = InboxKey::decode(&encoded).unwrap();
            prop_assert_eq!(key, decoded);
        }

        /// InboxKey encoding preserves ordering.
        #[test]
        fn inbox_key_ordering(agent_bytes: [u8; 32], seq1: u64, seq2: u64) {
            let agent_id = AgentId::new(agent_bytes);
            let key1 = InboxKey::new(agent_id, seq1).encode();
            let key2 = InboxKey::new(agent_id, seq2).encode();

            if seq1 < seq2 {
                prop_assert!(key1 < key2);
            } else if seq1 > seq2 {
                prop_assert!(key1 > key2);
            } else {
                prop_assert_eq!(key1, key2);
            }
        }
    }

    #[allow(deprecated)]
    #[test]
    fn agent_meta_key_roundtrip_all_fields() {
        let agent_id = AgentId::generate();

        for field in [
            MetaField::HeadSeq,
            MetaField::InboxHead,
            MetaField::InboxTail,
            MetaField::Status,
            MetaField::SchemaVersion,
        ] {
            let key = AgentMetaKey::new(agent_id, field);
            let encoded = key.encode();
            let decoded = AgentMetaKey::decode(&encoded).unwrap();
            assert_eq!(key, decoded);
        }
    }
}

// ============================================================================
// Serialization Tests
// ============================================================================

mod serialization_tests {
    use super::*;
    use aura_core::{
        ActionKind, AgentId, Decision, Proposal, ToolCall, ToolResult, Transaction, TransactionType,
    };
    use bytes::Bytes;

    proptest! {
        /// Transaction roundtrips through JSON.
        #[test]
        fn transaction_json_roundtrip(
            payload: Vec<u8>,
            tx_type in prop_oneof![
                Just(TransactionType::UserPrompt),
                Just(TransactionType::AgentMsg),
                Just(TransactionType::Trigger),
                Just(TransactionType::System),
            ]
        ) {
            let agent_id = AgentId::generate();
            let tx = Transaction::new_chained(
                agent_id,
                tx_type,
                Bytes::from(payload),
                None,
            );

            let json = serde_json::to_string(&tx).unwrap();
            let parsed: Transaction = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(tx, parsed);
        }

        /// ToolCall roundtrips through JSON.
        #[test]
        fn tool_call_json_roundtrip(tool: String, path: String) {
            prop_assume!(!tool.is_empty());
            let tool_call = ToolCall::new(&tool, serde_json::json!({ "path": path }));

            let json = serde_json::to_string(&tool_call).unwrap();
            let parsed: ToolCall = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(tool_call, parsed);
        }

        /// ToolResult roundtrips through JSON.
        #[test]
        fn tool_result_json_roundtrip(tool: String, output: String, ok: bool) {
            prop_assume!(!tool.is_empty());
            let result = if ok {
                ToolResult::success(&tool, output.clone())
            } else {
                ToolResult::failure(&tool, output.clone())
            };

            let json = serde_json::to_string(&result).unwrap();
            let parsed: ToolResult = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(result, parsed);
        }

        /// Proposal roundtrips through JSON.
        #[test]
        fn proposal_json_roundtrip(
            payload: Vec<u8>,
            kind in prop_oneof![
                Just(ActionKind::Reason),
                Just(ActionKind::Memorize),
                Just(ActionKind::Decide),
                Just(ActionKind::Delegate),
            ],
            rationale: Option<String>
        ) {
            let mut proposal = Proposal::new(kind, Bytes::from(payload));
            if let Some(r) = rationale {
                proposal = proposal.with_rationale(r);
            }

            let json = serde_json::to_string(&proposal).unwrap();
            let parsed: Proposal = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(proposal, parsed);
        }

        /// Decision roundtrips through JSON.
        #[test]
        fn decision_json_roundtrip(
            num_accepted in 0usize..5,
            num_rejected in 0usize..5
        ) {
            let mut decision = Decision::new();

            for _ in 0..num_accepted {
                decision.accept(aura_core::ActionId::generate());
            }

            for i in 0..num_rejected {
                decision.reject(i as u32, format!("Reason {i}"));
            }

            let json = serde_json::to_string(&decision).unwrap();
            let parsed: Decision = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(decision, parsed);
        }
    }
}

// ============================================================================
// Sandbox Path Tests
// ============================================================================

mod sandbox_tests {
    use super::*;
    use aura_tools::Sandbox;
    use tempfile::TempDir;

    // Strategy for generating "safe" relative paths
    fn safe_path_component() -> impl Strategy<Value = String> {
        prop::string::string_regex("[a-zA-Z0-9_-]{1,20}")
            .unwrap()
            .prop_filter("not empty", |s| !s.is_empty())
    }

    proptest! {
        /// Safe relative paths resolve within sandbox.
        #[test]
        fn safe_path_stays_in_sandbox(components in prop::collection::vec(safe_path_component(), 1..4)) {
            let dir = TempDir::new().unwrap();
            let sandbox = Sandbox::new(dir.path()).unwrap();

            let path = components.join("/");
            let resolved = sandbox.resolve(&path);

            // Should succeed (path is within sandbox)
            prop_assert!(resolved.is_ok());
            prop_assert!(resolved.unwrap().starts_with(sandbox.root()));
        }

        /// Paths with .. that escape are blocked.
        #[test]
        fn dotdot_escape_blocked(depth in 1usize..10) {
            let dir = TempDir::new().unwrap();
            let sandbox = Sandbox::new(dir.path()).unwrap();

            // Create a path that tries to escape
            let path = "../".repeat(depth) + "etc/passwd";
            let resolved = sandbox.resolve(&path);

            // Should fail (escapes sandbox)
            prop_assert!(resolved.is_err());
        }
    }
}

// ============================================================================
// Progress Bar Tests
// ============================================================================

mod progress_tests {
    use super::*;
    use aura_terminal::ProgressBar;

    proptest! {
        /// Progress value is clamped to [0, 1].
        #[test]
        fn progress_clamped(value: f32) {
            let bar = ProgressBar::new(20).with_progress(value);
            // We can't directly access progress, but render_string should not panic
            let rendered = bar.render_string();
            prop_assert!(!rendered.is_empty());
        }

        /// Progress bar renders without panicking for any width.
        #[test]
        fn progress_any_width(width in 5u16..200, progress in 0.0f32..=1.0) {
            let bar = ProgressBar::new(width).with_progress(progress);
            let rendered = bar.render_string();
            prop_assert!(!rendered.is_empty());
        }
    }
}

// ============================================================================
// Input History Tests
// ============================================================================

// Note: History tests are in aura-terminal's internal test module
// since InputHistory is not re-exported publicly

// ============================================================================
// Hash Chain Property Tests
// ============================================================================

mod hash_chain_tests {
    use super::*;
    use aura_core::{Hash, ProcessId};

    proptest! {
        /// Hash is deterministic - same content always produces same hash.
        #[test]
        fn prop_hash_deterministic(content: Vec<u8>) {
            let hash1 = Hash::from_content(&content);
            let hash2 = Hash::from_content(&content);
            prop_assert_eq!(hash1, hash2);
        }

        /// Hash with chaining is deterministic.
        #[test]
        fn prop_hash_chained_deterministic(content: Vec<u8>, prev_content: Vec<u8>) {
            let prev_hash = Hash::from_content(&prev_content);

            let hash1 = Hash::from_content_chained(&content, Some(&prev_hash));
            let hash2 = Hash::from_content_chained(&content, Some(&prev_hash));
            prop_assert_eq!(hash1, hash2);
        }

        /// Different chain orderings produce different hashes.
        #[test]
        fn prop_hash_chain_order_matters(content1: Vec<u8>, content2: Vec<u8>) {
            prop_assume!(!content1.is_empty() && !content2.is_empty());
            prop_assume!(content1 != content2);

            // Chain: content1 -> content2
            let hash1 = Hash::from_content(&content1);
            let chain1_final = Hash::from_content_chained(&content2, Some(&hash1));

            // Chain: content2 -> content1
            let hash2 = Hash::from_content(&content2);
            let chain2_final = Hash::from_content_chained(&content1, Some(&hash2));

            // Different orderings should produce different final hashes
            prop_assert_ne!(chain1_final, chain2_final);
        }

        /// Any content change changes the hash.
        #[test]
        fn prop_hash_content_sensitive(content1: Vec<u8>, content2: Vec<u8>) {
            prop_assume!(content1 != content2);

            let hash1 = Hash::from_content(&content1);
            let hash2 = Hash::from_content(&content2);
            prop_assert_ne!(hash1, hash2);
        }

        /// Chained hashes with different prev_hash produce different results.
        #[test]
        fn prop_hash_chain_depends_on_prev(content: Vec<u8>, prev1: Vec<u8>, prev2: Vec<u8>) {
            prop_assume!(prev1 != prev2);

            let prev_hash1 = Hash::from_content(&prev1);
            let prev_hash2 = Hash::from_content(&prev2);

            let chained1 = Hash::from_content_chained(&content, Some(&prev_hash1));
            let chained2 = Hash::from_content_chained(&content, Some(&prev_hash2));

            // Same content but different prev should produce different hash
            prop_assert_ne!(chained1, chained2);
        }

        /// Genesis (no prev) vs chained (with prev) produce different hashes.
        #[test]
        fn prop_genesis_differs_from_chained(content: Vec<u8>, prev_content: Vec<u8>) {
            let prev_hash = Hash::from_content(&prev_content);

            let genesis = Hash::from_content(&content);
            let chained = Hash::from_content_chained(&content, Some(&prev_hash));

            // Genesis and chained should differ (unless prev_hash happens to be all zeros,
            // but that's astronomically unlikely with random content)
            prop_assert_ne!(genesis, chained);
        }

        /// Hash hex roundtrip works for any content.
        #[test]
        fn prop_hash_hex_roundtrip(content: Vec<u8>) {
            let hash = Hash::from_content(&content);
            let hex = hash.to_hex();
            let parsed = Hash::from_hex(&hex).unwrap();
            prop_assert_eq!(hash, parsed);
        }

        /// ProcessId hex roundtrip works for any bytes.
        #[test]
        fn prop_process_id_hex_roundtrip(bytes: [u8; 16]) {
            let id = ProcessId::new(bytes);
            let hex = id.to_hex();
            let parsed = ProcessId::from_hex(&hex).unwrap();
            prop_assert_eq!(id, parsed);
        }
    }
}

// ============================================================================
// Transaction Chain Property Tests
// ============================================================================

mod transaction_chain_tests {
    use super::*;
    use aura_core::{AgentId, Transaction, TransactionType};

    proptest! {
        /// Transaction chain is append-only: modifying middle tx changes downstream hashes.
        #[test]
        fn prop_chain_is_append_only(
            msg1: Vec<u8>,
            msg2: Vec<u8>,
            msg3: Vec<u8>,
            tampered_msg1: Vec<u8>
        ) {
            prop_assume!(!msg1.is_empty() && !msg2.is_empty() && !msg3.is_empty());
            prop_assume!(msg1 != tampered_msg1);

            let agent_id = AgentId::generate();

            // Original chain
            let tx1 = Transaction::user_prompt(agent_id, msg1.clone());
            let tx2 = Transaction::user_prompt_chained(agent_id, msg2.clone(), &tx1.hash);
            let tx3 = Transaction::user_prompt_chained(agent_id, msg3.clone(), &tx2.hash);

            // Tampered chain (different first message)
            let tampered_tx1 = Transaction::user_prompt(agent_id, tampered_msg1);
            let tampered_tx2 = Transaction::user_prompt_chained(agent_id, msg2.clone(), &tampered_tx1.hash);
            let tampered_tx3 = Transaction::user_prompt_chained(agent_id, msg3.clone(), &tampered_tx2.hash);

            // All hashes should differ due to chain integrity
            prop_assert_ne!(tx1.hash, tampered_tx1.hash);
            prop_assert_ne!(tx2.hash, tampered_tx2.hash);
            prop_assert_ne!(tx3.hash, tampered_tx3.hash);
        }

        /// Transactions with reference_tx_hash maintain the reference correctly.
        #[test]
        fn prop_reference_tx_hash_preserved(payload: Vec<u8>) {
            use aura_core::{ActionId, ActionResultPayload, ProcessId};

            let agent_id = AgentId::generate();

            // Create original transaction
            let orig_tx = Transaction::user_prompt(agent_id, payload.clone());

            // Create a callback transaction that references the original
            // Using process_complete which sets reference_tx_hash
            let result_payload = ActionResultPayload::success(
                ActionId::generate(),
                ProcessId::generate(),
                Some(0),
                b"done".to_vec(),
                100,
            );
            let callback_tx = Transaction::process_complete(
                agent_id,
                &result_payload,
                orig_tx.hash,
                Some(&orig_tx.hash), // prev_hash
            )
            .unwrap();

            // Reference should be preserved
            prop_assert_eq!(callback_tx.reference_tx_hash, Some(orig_tx.hash));

            // Serialization roundtrip should preserve reference
            let json = serde_json::to_string(&callback_tx).unwrap();
            let parsed: Transaction = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(parsed.reference_tx_hash, Some(orig_tx.hash));
        }

        /// Same transaction content produces same hash (determinism).
        #[test]
        fn prop_transaction_deterministic(payload: Vec<u8>) {
            let agent_id = AgentId::generate();

            let tx1 = Transaction::user_prompt(agent_id, payload.clone());
            let tx2 = Transaction::user_prompt(agent_id, payload);

            prop_assert_eq!(tx1.hash, tx2.hash);
        }

        /// Chained transactions produce different hashes than genesis.
        #[test]
        fn prop_chained_differs_from_genesis(payload: Vec<u8>, prev_payload: Vec<u8>) {
            let agent_id = AgentId::generate();

            // Genesis transaction
            let genesis = Transaction::user_prompt(agent_id, payload.clone());

            // Create a prev transaction to chain from
            let prev_tx = Transaction::user_prompt(agent_id, prev_payload);

            // Chained transaction with same payload
            let chained = Transaction::user_prompt_chained(agent_id, payload, &prev_tx.hash);

            // Should produce different hashes
            prop_assert_ne!(genesis.hash, chained.hash);
        }

        /// Transaction type serialization is stable.
        #[test]
        fn prop_transaction_type_serialization_stable(
            tx_type in prop_oneof![
                Just(TransactionType::UserPrompt),
                Just(TransactionType::AgentMsg),
                Just(TransactionType::ActionResult),
                Just(TransactionType::Trigger),
                Just(TransactionType::System),
                Just(TransactionType::ProcessComplete),
            ]
        ) {
            let json1 = serde_json::to_string(&tx_type).unwrap();
            let parsed: TransactionType = serde_json::from_str(&json1).unwrap();
            let json2 = serde_json::to_string(&parsed).unwrap();

            prop_assert_eq!(json1, json2);
            prop_assert_eq!(tx_type, parsed);
        }
    }
}
