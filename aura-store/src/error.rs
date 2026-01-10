//! Storage error types.

use aura_core::{AgentId, TxId};
use thiserror::Error;

/// Storage-specific error type.
#[derive(Error, Debug)]
pub enum StoreError {
    #[error("RocksDB error: {0}")]
    RocksDb(#[from] rocksdb::Error),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("deserialization error: {0}")]
    Deserialization(String),

    #[error("agent not found: {0}")]
    AgentNotFound(AgentId),

    #[error("record entry not found: agent={0}, seq={1}")]
    RecordEntryNotFound(AgentId, u64),

    #[error("transaction not found: {0}")]
    TransactionNotFound(TxId),

    #[error("inbox empty for agent: {0}")]
    InboxEmpty(AgentId),

    #[error("sequence mismatch: expected {expected}, got {actual}")]
    SequenceMismatch { expected: u64, actual: u64 },

    #[error("column family not found: {0}")]
    ColumnFamilyNotFound(String),

    #[error("invalid key format: {0}")]
    InvalidKey(String),
}

impl From<serde_json::Error> for StoreError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialization(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_error_display() {
        let agent_id = AgentId::generate();

        let err = StoreError::AgentNotFound(agent_id);
        assert!(err.to_string().contains("agent not found"));

        let err = StoreError::RecordEntryNotFound(agent_id, 42);
        let display = err.to_string();
        assert!(display.contains("record entry not found"));
        assert!(display.contains("seq=42"));

        let err = StoreError::SequenceMismatch { expected: 10, actual: 5 };
        let display = err.to_string();
        assert!(display.contains("sequence mismatch"));
        assert!(display.contains("expected 10"));
        assert!(display.contains("got 5"));
    }

    #[test]
    fn test_store_error_serialization() {
        let err = StoreError::Serialization("invalid JSON".to_string());
        assert!(err.to_string().contains("serialization error"));
        assert!(err.to_string().contains("invalid JSON"));
    }

    #[test]
    fn test_store_error_deserialization() {
        let err = StoreError::Deserialization("missing field".to_string());
        assert!(err.to_string().contains("deserialization error"));
        assert!(err.to_string().contains("missing field"));
    }

    #[test]
    fn test_store_error_inbox_empty() {
        let agent_id = AgentId::generate();
        let err = StoreError::InboxEmpty(agent_id);
        assert!(err.to_string().contains("inbox empty"));
    }

    #[test]
    fn test_store_error_column_family_not_found() {
        let err = StoreError::ColumnFamilyNotFound("records".to_string());
        assert!(err.to_string().contains("column family not found"));
        assert!(err.to_string().contains("records"));
    }

    #[test]
    fn test_store_error_invalid_key() {
        let err = StoreError::InvalidKey("malformed key data".to_string());
        assert!(err.to_string().contains("invalid key format"));
        assert!(err.to_string().contains("malformed key data"));
    }

    #[test]
    fn test_store_error_from_serde_json() {
        // Create a serde_json error by parsing invalid JSON
        let json_err = serde_json::from_str::<serde_json::Value>("invalid json").unwrap_err();
        let store_err: StoreError = json_err.into();

        assert!(matches!(store_err, StoreError::Serialization(_)));
    }

    #[test]
    fn test_transaction_not_found() {
        let tx_id = TxId::from_content(b"test");
        let err = StoreError::TransactionNotFound(tx_id);
        assert!(err.to_string().contains("transaction not found"));
    }
}
