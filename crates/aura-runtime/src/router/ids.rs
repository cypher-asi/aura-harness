//! Shared identifier parsing helpers for router handlers.
//!
//! Phase 1 (refactor) extracts the duplicated `parse_agent_id` helpers
//! from `memory.rs`, `skills.rs`, `tool_permissions.rs`, and `tx.rs`
//! into one canonical implementation. The function accepts both UUID
//! strings (matching the memory + skills surface) and the 32-byte hex
//! form (matching the tx + tool-permissions surface) so every router
//! endpoint speaks the same agent-id grammar.

use super::errors::ApiError;
use aura_core::AgentId;

/// Parse an agent id from a path or body field.
///
/// Accepts either:
/// - a UUID string (8-4-4-4-12 hyphenated form), or
/// - a 32-byte lowercase hex string (the canonical [`AgentId::to_hex`]
///   round-trip).
///
/// Errors return [`ApiError::bad_request`] with `400 Bad Request` and a
/// JSON body `{ "error": "invalid agent_id: <reason>" }`.
pub(crate) fn parse_agent_id(s: &str) -> Result<AgentId, ApiError> {
    if let Ok(uuid) = uuid::Uuid::parse_str(s) {
        return Ok(AgentId::from_uuid(uuid));
    }
    AgentId::from_hex(s).map_err(|e| ApiError::bad_request(format!("invalid agent_id: {e}")))
}
