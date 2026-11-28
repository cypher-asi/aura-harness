//! Transaction chain property tests.

use super::*;
use aura_core_types::{AgentId, Transaction, TransactionType};

proptest! {
    /// Transaction chain is append-only: modifying middle tx changes downstream hashes.
    #[test]
    fn prop_chain_is_append_only(
        msg1: String,
        msg2: String,
        msg3: String,
        tampered_msg1: String
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
    fn prop_reference_tx_hash_preserved(payload: String) {
        use aura_core_types::{ActionId, ActionResultPayload, ProcessId};

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
    fn prop_transaction_deterministic(payload: String) {
        let agent_id = AgentId::generate();

        let tx1 = Transaction::user_prompt(agent_id, payload.clone());
        let tx2 = Transaction::user_prompt(agent_id, payload);

        prop_assert_eq!(tx1.hash, tx2.hash);
    }

    /// Chained transactions produce different hashes than genesis.
    #[test]
    fn prop_chained_differs_from_genesis(payload: String, prev_payload: String) {
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
