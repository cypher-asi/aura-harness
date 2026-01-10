//! Tool error types.

use thiserror::Error;

/// Tool-specific error type.
#[derive(Error, Debug)]
pub enum ToolError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),

    #[error("tool disabled: {0}")]
    ToolDisabled(String),

    #[error("sandbox violation: path {path} escapes workspace root")]
    SandboxViolation { path: String },

    #[error("path not found: {0}")]
    PathNotFound(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid arguments: {0}")]
    InvalidArguments(String),

    #[error("command not allowed: {0}")]
    CommandNotAllowed(String),

    #[error("command timeout after {timeout_ms}ms")]
    CommandTimeout { timeout_ms: u64 },

    #[error("command failed: {0}")]
    CommandFailed(String),

    #[error("size limit exceeded: {actual} > {limit}")]
    SizeLimitExceeded { actual: usize, limit: usize },

    #[error("serialization error: {0}")]
    Serialization(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_error_unknown_tool() {
        let err = ToolError::UnknownTool("mystery_tool".to_string());
        assert!(err.to_string().contains("unknown tool"));
        assert!(err.to_string().contains("mystery_tool"));
    }

    #[test]
    fn test_tool_error_tool_disabled() {
        let err = ToolError::ToolDisabled("cmd_run".to_string());
        assert!(err.to_string().contains("tool disabled"));
        assert!(err.to_string().contains("cmd_run"));
    }

    #[test]
    fn test_tool_error_sandbox_violation() {
        let err = ToolError::SandboxViolation {
            path: "../../../etc/passwd".to_string(),
        };
        let display = err.to_string();
        assert!(display.contains("sandbox violation"));
        assert!(display.contains("../../../etc/passwd"));
        assert!(display.contains("escapes workspace root"));
    }

    #[test]
    fn test_tool_error_path_not_found() {
        let err = ToolError::PathNotFound("/nonexistent/file.txt".to_string());
        assert!(err.to_string().contains("path not found"));
        assert!(err.to_string().contains("/nonexistent/file.txt"));
    }

    #[test]
    fn test_tool_error_invalid_arguments() {
        let err = ToolError::InvalidArguments("missing required field 'path'".to_string());
        assert!(err.to_string().contains("invalid arguments"));
        assert!(err.to_string().contains("missing required field"));
    }

    #[test]
    fn test_tool_error_command_not_allowed() {
        let err = ToolError::CommandNotAllowed("rm".to_string());
        assert!(err.to_string().contains("command not allowed"));
        assert!(err.to_string().contains("rm"));
    }

    #[test]
    fn test_tool_error_command_timeout() {
        let err = ToolError::CommandTimeout { timeout_ms: 30000 };
        let display = err.to_string();
        assert!(display.contains("command timeout"));
        assert!(display.contains("30000"));
    }

    #[test]
    fn test_tool_error_command_failed() {
        let err = ToolError::CommandFailed("exit code 1".to_string());
        assert!(err.to_string().contains("command failed"));
        assert!(err.to_string().contains("exit code 1"));
    }

    #[test]
    fn test_tool_error_size_limit_exceeded() {
        let err = ToolError::SizeLimitExceeded {
            actual: 10_000_000,
            limit: 5_000_000,
        };
        let display = err.to_string();
        assert!(display.contains("size limit exceeded"));
        assert!(display.contains("10000000"));
        assert!(display.contains("5000000"));
    }

    #[test]
    fn test_tool_error_serialization() {
        let err = ToolError::Serialization("invalid JSON structure".to_string());
        assert!(err.to_string().contains("serialization error"));
        assert!(err.to_string().contains("invalid JSON structure"));
    }

    #[test]
    fn test_tool_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let tool_err: ToolError = io_err.into();

        assert!(matches!(tool_err, ToolError::Io(_)));
        assert!(tool_err.to_string().contains("io error"));
    }
}
