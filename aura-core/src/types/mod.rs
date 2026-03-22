//! Domain types for the Aura system.
//!
//! Includes Transaction, Action, Effect, `RecordEntry`, and related types.

mod action;
mod effect;
mod identity;
mod process;
mod proposal;
mod record;
mod tool;
mod transaction;

pub use action::{Action, ActionKind};
pub use effect::{Effect, EffectKind, EffectStatus};
pub use identity::Identity;
pub use process::{ActionResultPayload, ProcessPending};
pub use proposal::{Decision, Proposal, ProposalSet, RejectedProposal, Trace};
pub use record::{RecordEntry, RecordEntryBuilder, KERNEL_VERSION};
pub use tool::{
    ExternalToolDefinition, ToolCall, ToolDecision, ToolExecution, ToolProposal, ToolResult,
};
pub use transaction::{Transaction, TransactionType};

#[allow(deprecated)]
pub use transaction::TransactionKind;

// ============================================================================
// Serialization Helpers (shared across submodules)
// ============================================================================

/// Helper module for hex serialization of 32-byte arrays.
mod hex_bytes_32 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 32], D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom("expected 32 bytes"))
    }
}

/// Helper module for Bytes serialization as base64.
mod bytes_serde {
    use bytes::Bytes;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &Bytes, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        serializer.serialize_str(&encoded)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Bytes, D::Error>
    where
        D: Deserializer<'de>,
    {
        use base64::Engine;
        let s = String::deserialize(deserializer)?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&s)
            .map_err(serde::de::Error::custom)?;
        Ok(Bytes::from(decoded))
    }
}

/// Helper module for hex serialization of Hash type.
mod hex_hash {
    use crate::ids::Hash;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(hash: &Hash, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&hash.to_hex())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Hash, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Hash::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

/// Helper module for optional hex serialization of Hash type.
mod option_hex_hash {
    use crate::ids::Hash;
    use serde::{Deserialize, Deserializer, Serializer};

    #[allow(clippy::ref_option)]
    pub fn serialize<S>(hash: &Option<Hash>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match hash {
            Some(h) => serializer.serialize_some(&h.to_hex()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Hash>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<String> = Option::deserialize(deserializer)?;
        opt.map_or_else(
            || Ok(None),
            |s| {
                Hash::from_hex(&s)
                    .map(Some)
                    .map_err(serde::de::Error::custom)
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{ActionId, AgentId, ProcessId};

    #[test]
    fn transaction_roundtrip() {
        let tx = Transaction::user_prompt(AgentId::generate(), b"Hello, agent!".to_vec());
        let json = serde_json::to_string(&tx).unwrap();
        let parsed: Transaction = serde_json::from_str(&json).unwrap();
        assert_eq!(tx, parsed);
    }

    #[test]
    fn transaction_with_reference() {
        let agent_id = AgentId::generate();
        let orig_tx = Transaction::user_prompt(agent_id, b"start process".to_vec());
        let result_payload = ActionResultPayload::success(
            ActionId::generate(),
            ProcessId::generate(),
            Some(0),
            b"output".to_vec(),
            1000,
        );
        let callback_tx = Transaction::process_complete(
            agent_id,
            &result_payload,
            orig_tx.hash,
            Some(&orig_tx.hash),
        )
        .unwrap();

        assert_eq!(callback_tx.reference_tx_hash, Some(orig_tx.hash));
        assert_eq!(callback_tx.tx_type, TransactionType::ProcessComplete);

        let json = serde_json::to_string(&callback_tx).unwrap();
        let parsed: Transaction = serde_json::from_str(&json).unwrap();
        assert_eq!(callback_tx, parsed);
    }

    #[test]
    fn transaction_chaining() {
        let agent_id = AgentId::generate();

        let tx1 = Transaction::user_prompt(agent_id, b"first".to_vec());
        let tx2 = Transaction::user_prompt_chained(agent_id, b"second".to_vec(), &tx1.hash);

        let tx3 = Transaction::user_prompt(agent_id, b"second".to_vec());
        assert_ne!(tx2.hash, tx3.hash);

        let tx4 = Transaction::user_prompt_chained(agent_id, b"second".to_vec(), &tx1.hash);
        assert_eq!(tx2.hash, tx4.hash);
    }

    #[test]
    fn action_roundtrip() {
        let action = Action::new(
            ActionId::generate(),
            ActionKind::Delegate,
            b"tool payload".to_vec(),
        );
        let json = serde_json::to_string(&action).unwrap();
        let parsed: Action = serde_json::from_str(&json).unwrap();
        assert_eq!(action, parsed);
    }

    #[test]
    fn effect_roundtrip() {
        let effect = Effect::committed_agreement(ActionId::generate(), b"result".to_vec());
        let json = serde_json::to_string(&effect).unwrap();
        let parsed: Effect = serde_json::from_str(&json).unwrap();
        assert_eq!(effect, parsed);
    }

    #[test]
    fn record_entry_roundtrip() {
        let tx = Transaction::user_prompt(AgentId::generate(), b"test".to_vec());
        let entry = RecordEntry::builder(1, tx)
            .context_hash([1u8; 32])
            .proposals(ProposalSet::new())
            .decision(Decision::new())
            .actions(vec![])
            .effects(vec![])
            .build();

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: RecordEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, parsed);
    }

    #[test]
    fn identity_creation() {
        let identity = Identity::new("0://TestAgent", "Test Agent");
        assert!(!identity.zns_id.is_empty());
        assert_eq!(identity.name, "Test Agent");
    }

    #[test]
    fn tool_call_roundtrip() {
        let tool_call = ToolCall::fs_read("src/main.rs", Some(1024));
        let json = serde_json::to_string(&tool_call).unwrap();
        let parsed: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(tool_call, parsed);
    }

    #[test]
    fn tool_result_roundtrip() {
        let result =
            ToolResult::success("fs_read", b"file contents".to_vec()).with_metadata("size", "13");
        let json = serde_json::to_string(&result).unwrap();
        let parsed: ToolResult = serde_json::from_str(&json).unwrap();
        assert_eq!(result, parsed);
    }

    #[test]
    fn process_pending_roundtrip() {
        let pending = ProcessPending::new(ProcessId::generate(), "cargo build --release");
        let json = serde_json::to_string(&pending).unwrap();
        let parsed: ProcessPending = serde_json::from_str(&json).unwrap();
        assert_eq!(pending, parsed);
    }

    #[test]
    fn action_result_payload_success_roundtrip() {
        let payload = ActionResultPayload::success(
            ActionId::generate(),
            ProcessId::generate(),
            Some(0),
            b"build succeeded".to_vec(),
            5000,
        );
        let json = serde_json::to_string(&payload).unwrap();
        let parsed: ActionResultPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(payload, parsed);
        assert!(payload.success);
    }

    #[test]
    fn action_result_payload_failure_roundtrip() {
        let payload = ActionResultPayload::failure(
            ActionId::generate(),
            ProcessId::generate(),
            Some(1),
            b"build failed".to_vec(),
            3000,
        );
        let json = serde_json::to_string(&payload).unwrap();
        let parsed: ActionResultPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(payload, parsed);
        assert!(!payload.success);
    }

    #[test]
    fn transaction_type_serialization() {
        let types = vec![
            TransactionType::UserPrompt,
            TransactionType::AgentMsg,
            TransactionType::Trigger,
            TransactionType::ActionResult,
            TransactionType::System,
            TransactionType::SessionStart,
            TransactionType::ToolProposal,
            TransactionType::ToolExecution,
            TransactionType::ProcessComplete,
        ];

        for tx_type in types {
            let json = serde_json::to_string(&tx_type).unwrap();
            let parsed: TransactionType = serde_json::from_str(&json).unwrap();
            assert_eq!(tx_type, parsed);
        }
    }
}
