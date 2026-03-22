//! Error types for the Aura system.
//!
//! Uses `thiserror` for library errors with context preservation.

use crate::ids::{ActionId, AgentId, TxId};
use thiserror::Error;

/// Result type alias using `AuraError`.
pub type Result<T> = std::result::Result<T, AuraError>;

/// Core error type for the Aura system.
#[derive(Error, Debug)]
pub enum AuraError {
    // === Storage Errors ===
    #[error("storage error: {message}")]
    Storage {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("agent not found: {agent_id}")]
    AgentNotFound { agent_id: AgentId },

    #[error("record entry not found: agent={agent_id}, seq={seq}")]
    RecordEntryNotFound { agent_id: AgentId, seq: u64 },

    #[error("transaction not found: {tx_id}")]
    TransactionNotFound { tx_id: TxId },

    #[error("inbox empty for agent: {agent_id}")]
    InboxEmpty { agent_id: AgentId },

    #[error("sequence mismatch: expected {expected}, got {actual}")]
    SequenceMismatch { expected: u64, actual: u64 },

    // === Serialization Errors ===
    #[error("serialization error: {message}")]
    Serialization {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("deserialization error: {message}")]
    Deserialization {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    // === Kernel Errors ===
    #[error("kernel error: {message}")]
    Kernel { message: String },

    #[error("policy violation: {reason}")]
    PolicyViolation { reason: String },

    #[error("action not allowed: {action_kind}")]
    ActionNotAllowed { action_kind: String },

    #[error("tool not allowed: {tool}")]
    ToolNotAllowed { tool: String },

    // === Executor Errors ===
    #[error("executor error: {message}")]
    Executor { message: String },

    #[error("tool execution failed: {tool}, reason: {reason}")]
    ToolExecutionFailed { tool: String, reason: String },

    #[error("tool timeout: {tool}, timeout_ms: {timeout_ms}")]
    ToolTimeout { tool: String, timeout_ms: u64 },

    #[error("sandbox violation: {path}")]
    SandboxViolation { path: String },

    // === Reasoner Errors ===
    #[error("reasoner error: {message}")]
    Reasoner { message: String },

    #[error("reasoner timeout after {timeout_ms}ms")]
    ReasonerTimeout { timeout_ms: u64 },

    #[error("reasoner unavailable: {reason}")]
    ReasonerUnavailable { reason: String },

    // === Validation Errors ===
    #[error("validation error: {message}")]
    Validation { message: String },

    #[error("invalid transaction: {reason}")]
    InvalidTransaction { reason: String },

    #[error("invalid action: {action_id}, reason: {reason}")]
    InvalidAction { action_id: ActionId, reason: String },

    // === Configuration Errors ===
    #[error("configuration error: {message}")]
    Configuration { message: String },

    // === Internal Errors ===
    #[error("internal error: {message}")]
    Internal { message: String },
}

impl AuraError {
    /// Create a storage error with a message.
    pub fn storage(message: impl Into<String>) -> Self {
        Self::Storage {
            message: message.into(),
            source: None,
        }
    }

    /// Create a storage error with a source.
    pub fn storage_with_source(
        message: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Storage {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }

    /// Create a serialization error.
    pub fn serialization(message: impl Into<String>) -> Self {
        Self::Serialization {
            message: message.into(),
            source: None,
        }
    }

    /// Create a serialization error with a source.
    pub fn serialization_with_source(
        message: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Serialization {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }

    /// Create a deserialization error.
    pub fn deserialization(message: impl Into<String>) -> Self {
        Self::Deserialization {
            message: message.into(),
            source: None,
        }
    }

    /// Create a deserialization error with a source.
    pub fn deserialization_with_source(
        message: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Deserialization {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }

    /// Create a kernel error.
    pub fn kernel(message: impl Into<String>) -> Self {
        Self::Kernel {
            message: message.into(),
        }
    }

    /// Create a policy violation error.
    pub fn policy_violation(reason: impl Into<String>) -> Self {
        Self::PolicyViolation {
            reason: reason.into(),
        }
    }

    /// Create an executor error.
    pub fn executor(message: impl Into<String>) -> Self {
        Self::Executor {
            message: message.into(),
        }
    }

    /// Create a reasoner error.
    pub fn reasoner(message: impl Into<String>) -> Self {
        Self::Reasoner {
            message: message.into(),
        }
    }

    /// Create a validation error.
    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation {
            message: message.into(),
        }
    }

    /// Create a configuration error.
    pub fn configuration(message: impl Into<String>) -> Self {
        Self::Configuration {
            message: message.into(),
        }
    }

    /// Create an internal error.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
        }
    }
}

// Conversion from serde_json errors
impl From<serde_json::Error> for AuraError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialization {
            message: err.to_string(),
            source: Some(Box::new(err)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = AuraError::storage("test storage error");
        assert!(err.to_string().contains("storage error"));
        assert!(err.to_string().contains("test storage error"));
    }

    #[test]
    fn test_storage_error() {
        let err = AuraError::storage("disk full");
        assert!(
            matches!(err, AuraError::Storage { message, source: None } if message == "disk full")
        );
    }

    #[test]
    fn test_storage_error_with_source() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err = AuraError::storage_with_source("read failed", io_err);

        match err {
            AuraError::Storage { message, source } => {
                assert_eq!(message, "read failed");
                assert!(source.is_some());
            }
            _ => panic!("Expected Storage error"),
        }
    }

    #[test]
    fn test_agent_not_found() {
        let agent_id = AgentId::new([42u8; 32]);
        let err = AuraError::AgentNotFound { agent_id };

        let display = err.to_string();
        assert!(display.contains("agent not found"));
    }

    #[test]
    fn test_record_entry_not_found() {
        let agent_id = AgentId::new([1u8; 32]);
        let err = AuraError::RecordEntryNotFound { agent_id, seq: 42 };

        let display = err.to_string();
        assert!(display.contains("record entry not found"));
        assert!(display.contains("seq=42"));
    }

    #[test]
    fn test_sequence_mismatch() {
        let err = AuraError::SequenceMismatch {
            expected: 10,
            actual: 5,
        };

        let display = err.to_string();
        assert!(display.contains("sequence mismatch"));
        assert!(display.contains("expected 10"));
        assert!(display.contains("got 5"));
    }

    #[test]
    fn test_serialization_error() {
        let err = AuraError::serialization("invalid JSON");
        assert!(
            matches!(err, AuraError::Serialization { message, source: None } if message == "invalid JSON")
        );
    }

    #[test]
    fn test_deserialization_error() {
        let err = AuraError::deserialization("missing field");
        assert!(
            matches!(err, AuraError::Deserialization { message, source: None } if message == "missing field")
        );
    }

    #[test]
    fn test_policy_violation() {
        let err = AuraError::policy_violation("tool not allowed");

        let display = err.to_string();
        assert!(display.contains("policy violation"));
        assert!(display.contains("tool not allowed"));
    }

    #[test]
    fn test_tool_not_allowed() {
        let err = AuraError::ToolNotAllowed {
            tool: "dangerous_tool".to_string(),
        };

        let display = err.to_string();
        assert!(display.contains("tool not allowed"));
        assert!(display.contains("dangerous_tool"));
    }

    #[test]
    fn test_tool_execution_failed() {
        let err = AuraError::ToolExecutionFailed {
            tool: "fs_read".to_string(),
            reason: "permission denied".to_string(),
        };

        let display = err.to_string();
        assert!(display.contains("tool execution failed"));
        assert!(display.contains("fs_read"));
        assert!(display.contains("permission denied"));
    }

    #[test]
    fn test_tool_timeout() {
        let err = AuraError::ToolTimeout {
            tool: "cmd_run".to_string(),
            timeout_ms: 30000,
        };

        let display = err.to_string();
        assert!(display.contains("tool timeout"));
        assert!(display.contains("30000"));
    }

    #[test]
    fn test_sandbox_violation() {
        let err = AuraError::SandboxViolation {
            path: "../../../etc/passwd".to_string(),
        };

        let display = err.to_string();
        assert!(display.contains("sandbox violation"));
        assert!(display.contains("../../../etc/passwd"));
    }

    #[test]
    fn test_reasoner_timeout() {
        let err = AuraError::ReasonerTimeout { timeout_ms: 60000 };

        let display = err.to_string();
        assert!(display.contains("reasoner timeout"));
        assert!(display.contains("60000"));
    }

    #[test]
    fn test_invalid_transaction() {
        let err = AuraError::InvalidTransaction {
            reason: "empty payload".to_string(),
        };

        let display = err.to_string();
        assert!(display.contains("invalid transaction"));
        assert!(display.contains("empty payload"));
    }

    #[test]
    fn test_invalid_action() {
        let action_id = ActionId::new([7u8; 16]);
        let err = AuraError::InvalidAction {
            action_id,
            reason: "malformed payload".to_string(),
        };

        let display = err.to_string();
        assert!(display.contains("invalid action"));
        assert!(display.contains("malformed payload"));
    }

    #[test]
    fn test_from_serde_json_error() {
        // Create an invalid JSON to generate a serde_json error
        let json_err = serde_json::from_str::<serde_json::Value>("invalid json").unwrap_err();
        let aura_err: AuraError = json_err.into();

        assert!(matches!(aura_err, AuraError::Serialization { .. }));
    }

    #[test]
    fn test_error_helper_functions() {
        // Test all helper functions create expected error types
        assert!(matches!(
            AuraError::kernel("test"),
            AuraError::Kernel { .. }
        ));
        assert!(matches!(
            AuraError::executor("test"),
            AuraError::Executor { .. }
        ));
        assert!(matches!(
            AuraError::reasoner("test"),
            AuraError::Reasoner { .. }
        ));
        assert!(matches!(
            AuraError::validation("test"),
            AuraError::Validation { .. }
        ));
        assert!(matches!(
            AuraError::configuration("test"),
            AuraError::Configuration { .. }
        ));
        assert!(matches!(
            AuraError::internal("test"),
            AuraError::Internal { .. }
        ));
    }

    #[test]
    fn test_result_type_alias() {
        fn returns_result() -> Result<i32> {
            Ok(42)
        }

        fn returns_error() -> Result<i32> {
            Err(AuraError::internal("test"))
        }

        assert_eq!(returns_result().unwrap(), 42);
        assert!(returns_error().is_err());
    }

    #[test]
    fn test_deserialization_error_with_source() {
        let io_err = std::io::Error::new(std::io::ErrorKind::InvalidData, "bad data");
        let err = AuraError::deserialization_with_source("parse failed", io_err);
        match err {
            AuraError::Deserialization { message, source } => {
                assert_eq!(message, "parse failed");
                assert!(source.is_some());
            }
            _ => panic!("Expected Deserialization error"),
        }
    }

    #[test]
    fn test_serialization_with_source() {
        let io_err = std::io::Error::new(std::io::ErrorKind::Other, "write failed");
        let err = AuraError::serialization_with_source("encode failed", io_err);
        match err {
            AuraError::Serialization { message, source } => {
                assert_eq!(message, "encode failed");
                assert!(source.is_some());
            }
            _ => panic!("Expected Serialization error"),
        }
    }

    #[test]
    fn test_error_display_contains_message_for_all_variants() {
        let cases: Vec<(AuraError, &str)> = vec![
            (AuraError::storage("msg"), "storage error: msg"),
            (AuraError::kernel("msg"), "kernel error: msg"),
            (AuraError::executor("msg"), "executor error: msg"),
            (AuraError::reasoner("msg"), "reasoner error: msg"),
            (AuraError::validation("msg"), "validation error: msg"),
            (AuraError::configuration("msg"), "configuration error: msg"),
            (AuraError::internal("msg"), "internal error: msg"),
            (AuraError::serialization("msg"), "serialization error: msg"),
            (AuraError::deserialization("msg"), "deserialization error: msg"),
            (AuraError::policy_violation("msg"), "policy violation: msg"),
        ];

        for (err, expected) in cases {
            assert_eq!(err.to_string(), expected);
        }
    }

    #[test]
    fn test_error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AuraError>();
    }

    #[test]
    fn test_reasoner_unavailable() {
        let err = AuraError::ReasonerUnavailable {
            reason: "rate limited".to_string(),
        };
        let display = err.to_string();
        assert!(display.contains("reasoner unavailable"));
        assert!(display.contains("rate limited"));
    }

    #[test]
    fn test_action_not_allowed() {
        let err = AuraError::ActionNotAllowed {
            action_kind: "Delegate".to_string(),
        };
        let display = err.to_string();
        assert!(display.contains("action not allowed"));
        assert!(display.contains("Delegate"));
    }

    #[test]
    fn test_inbox_empty() {
        let agent_id = AgentId::new([0u8; 32]);
        let err = AuraError::InboxEmpty { agent_id };
        let display = err.to_string();
        assert!(display.contains("inbox empty"));
    }

    #[test]
    fn test_error_debug_format() {
        let err = AuraError::internal("debug test");
        let debug_str = format!("{err:?}");
        assert!(debug_str.contains("Internal"));
        assert!(debug_str.contains("debug test"));
    }

    #[test]
    fn test_from_serde_json_preserves_source() {
        let json_err = serde_json::from_str::<serde_json::Value>("{invalid").unwrap_err();
        let err_msg = json_err.to_string();
        let aura_err: AuraError = json_err.into();
        match aura_err {
            AuraError::Serialization { message, source } => {
                assert_eq!(message, err_msg);
                assert!(source.is_some());
            }
            _ => panic!("Expected Serialization error"),
        }
    }
}
