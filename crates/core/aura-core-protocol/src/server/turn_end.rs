use serde::{Deserialize, Serialize};

#[cfg(feature = "typescript")]
use ts_rs::TS;

use crate::context::ContextContents;

/// Payload for `assistant_message_end`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct AssistantMessageEnd {
    pub message_id: String,
    pub stop_reason: String,
    pub usage: SessionUsage,
    pub files_changed: FilesChanged,
    /// Phase 5 billing roll-up: the originating user whose budget
    /// should absorb this turn's cost. When `None`, the immediate
    /// agent owner is billed (today's behavior). Populated by the
    /// harness when a spawned agent's work should roll up to the
    /// ancestor user via `walk_parent_chain`. Strictly additive —
    /// older harness builds never set this field; older clients
    /// ignore it on deserialize.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub originating_user_id: Option<String>,
}

/// Token usage information for a session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct SessionUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_context_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cumulative_input_tokens: u64,
    pub cumulative_output_tokens: u64,
    pub cumulative_cache_creation_input_tokens: u64,
    pub cumulative_cache_read_input_tokens: u64,
    /// Fraction of the model's context window consumed (0.0–1.0).
    pub context_utilization: f32,
    /// Model identifier used for this turn.
    pub model: String,
    /// Provider name (e.g., "anthropic").
    pub provider: String,
    /// Per-bucket token estimates that sum (approximately) to
    /// `estimated_context_tokens`. Strictly additive — older harness
    /// builds emit `ContextBreakdown::default()` (all zeros), and the
    /// frontend treats an all-zero breakdown as "not available" and
    /// falls back to the legacy used/total view.
    #[serde(default)]
    pub context_breakdown: ContextBreakdown,
    /// Actual rendered text for each static context bucket, parallel to
    /// [`Self::context_breakdown`]'s token counts. Strictly additive:
    /// older harness builds omit this field (`None`), and the wire
    /// boundary only sets it `Some(_)` when at least one bucket carries
    /// content, so empty turns stay omitted on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_contents: Option<ContextContents>,
}

/// Per-bucket token estimates for the current session context, computed
/// using the same `chars / CHARS_PER_TOKEN` heuristic as
/// [`SessionUsage::estimated_context_tokens`]. The buckets approximate
/// what the model actually receives on the next turn:
///
/// - `system_prompt_tokens` — the rendered system prompt.
/// - `tools_tokens` — serialized tool definitions (name + description +
///   JSON schema for each tool the request would carry).
/// - `skills_tokens` — installed skill metadata (name + description).
/// - `mcp_tokens` — reserved for MCP server context once aura-harness
///   gains MCP support; today this is always `0`.
/// - `subagents_tokens` — registered subagent kind specs.
/// - `conversation_tokens` — the live message transcript including
///   tool results and assistant turns. This is the same number as
///   `estimated_context_tokens` minus the static buckets above.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct ContextBreakdown {
    pub system_prompt_tokens: u64,
    pub tools_tokens: u64,
    pub skills_tokens: u64,
    pub mcp_tokens: u64,
    pub subagents_tokens: u64,
    pub conversation_tokens: u64,
    /// Tokens served from the upstream provider's prompt cache during
    /// the most recent turn (Anthropic's `cache_read_input_tokens` or
    /// OpenAI's `prompt_tokens_details.cached_tokens`). Describes what
    /// fraction of the *conversation* bucket was a cache hit; not a
    /// separate context bucket, so excluded from [`Self::total`] and
    /// [`Self::is_empty`].
    #[serde(default)]
    pub cache_read_tokens: u64,
    /// Tokens written to the upstream provider's prompt cache during
    /// the most recent turn (Anthropic's `cache_creation_input_tokens`,
    /// or the cache-miss portion of OpenAI's responses). See
    /// [`Self::cache_read_tokens`] for why this is NOT included in
    /// [`Self::total`] / [`Self::is_empty`].
    #[serde(default)]
    pub cache_creation_tokens: u64,
}

impl ContextBreakdown {
    /// True when every bucket is zero. Used by the frontend to detect
    /// pre-upgrade harness builds and fall back to the legacy popover.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.system_prompt_tokens == 0
            && self.tools_tokens == 0
            && self.skills_tokens == 0
            && self.mcp_tokens == 0
            && self.subagents_tokens == 0
            && self.conversation_tokens == 0
    }

    /// Sum of every bucket. Useful as a sanity check against
    /// [`SessionUsage::estimated_context_tokens`].
    #[must_use]
    pub fn total(&self) -> u64 {
        self.system_prompt_tokens
            .saturating_add(self.tools_tokens)
            .saturating_add(self.skills_tokens)
            .saturating_add(self.mcp_tokens)
            .saturating_add(self.subagents_tokens)
            .saturating_add(self.conversation_tokens)
    }
}

/// A single file mutation observed during a turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct FileOp {
    pub path: String,
    pub operation: String,
}

/// Per-file diff metadata that runs alongside the path lists in
/// [`FilesChanged`]. Optional and additive: senders that don't compute
/// line counts (older harnesses, write/delete tool paths) simply omit
/// the entry, and consumers must treat "no entry for path" / "0 / 0"
/// as "unknown", not "no change".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct FileDiff {
    pub path: String,
    pub lines_added: u32,
    pub lines_removed: u32,
}

/// Summary of file mutations during a turn.
///
/// `diffs` is a parallel list of per-path line-count metadata. It lives
/// alongside `created` / `modified` / `deleted` (rather than replacing
/// them) so older clients that only know about the path lists keep
/// working untouched. The serde default + skip_if_empty pair keeps the
/// wire format byte-identical when no diffs are computed, preserving
/// JSON-shape compatibility with any pinned schemas.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct FilesChanged {
    pub created: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diffs: Vec<FileDiff>,
}

impl FilesChanged {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.created.is_empty() && self.modified.is_empty() && self.deleted.is_empty()
    }
}
