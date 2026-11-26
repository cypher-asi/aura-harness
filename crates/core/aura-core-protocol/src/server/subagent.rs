use serde::{Deserialize, Serialize};

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// Payload for `subagent_spawned`.
///
/// Announces a child subagent run on the parent stream. `child_run_id`
/// is a freshly minted run id the client can attach to via
/// `WS /stream/:child_run_id`. `parent_tool_use_id` ties the thread
/// back to the originating `task` tool-use block so the UI can render
/// the live thread under that tool card.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct SubagentSpawned {
    pub child_run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_tool_use_id: Option<String>,
    pub subagent_type: String,
    pub prompt: String,
    /// Model id driving this child run. Set for AURA Council members so
    /// the UI can label each column; `None` for ordinary `task` spawns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Zero-based council slot index for AURA Council members (ordering
    /// the columns); `None` for ordinary `task` spawns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub council_index: Option<u32>,
    /// Council combine mechanism (`synthesize` / `contrast` /
    /// `side_by_side`) shared by every member of the turn. Set on AURA
    /// Council member spawns so the UI can label the panel with the
    /// active mechanism; `None` for ordinary `task` spawns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub council_mechanism: Option<String>,
}

/// Payload for `subagent_status`.
///
/// `state` is one of `running | completed | failed | cancelled |
/// timeout | rejected`. `reason` carries the failure/rejection detail
/// when applicable (depth/quota rejections surface here).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS), ts(export))]
pub struct SubagentStatus {
    pub child_run_id: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
