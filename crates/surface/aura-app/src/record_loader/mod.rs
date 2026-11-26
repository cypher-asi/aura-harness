//! Loading and displaying historical records from the store.

use crate::agent_registry::AgentRegistry;
use crate::event_loop::run_command::is_user_tool_use;
use crate::turn_calls::{Seen, TurnCalls};
use aura_agent_kernel::decode_tool_effect;
use aura_core_types::{
    ActionResultPayload, AgentId, EffectStatus, PlanUsage, PolicyApprover, PolicyCheckOutcome,
    PolicyDecision, RecordEntry, ToolExecution, ToolGateVerdict, ToolProposal, Transaction,
    TransactionType,
};
use aura_exec_tools::plan_tools::{plan_path_from_output, CREATE_PLAN_TOOL_NAME};
use aura_exec_tools::todo::TODO_WRITE_TOOL_NAME;
use aura_gate::compare::ModelCall;
use aura_model::{Message, ModelCallPurpose, Usage};
use aura_store_record::{ReadStore, StoreError};
use aura_ui::{
    events::{
        ActivityData, AgentSummary, MessageData, MessageRole, PolicyDetail, ProjectSummary,
        RecordStatus, RecordSummary, MAX_RECORDS,
    },
    UiCommand,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::mpsc;
use tracing::warn;

/// Number of entries loaded into the Record panel: as many as it holds.
const RECORD_PANEL_LIMIT: usize = MAX_RECORDS;

/// Number of trailing entries scanned when rebuilding an agent's chat.
const HISTORY_SCAN_LIMIT: usize = 1000;

/// Number of chat messages kept after rebuilding, activity blocks included
/// (matches the UI cap).
pub(crate) const HISTORY_MESSAGE_LIMIT: usize = 300;

/// Upper bound on the summary string; the panel truncates to its own
/// width, this only keeps huge prompts from being copied around.
const MAX_INFO_CHARS: usize = 240;

/// Upper bound on a plain-text payload shown in the record detail view.
const MAX_MESSAGE_CHARS: usize = 200;

/// Upper bound on a pretty-printed JSON payload; the detail view scrolls,
/// so this only guards against huge tool payloads.
const MAX_JSON_MESSAGE_CHARS: usize = 20_000;

mod conversation;
mod format;
mod history;
mod lists;
mod policy;
mod summary;

pub use conversation::*;
pub use format::*;
pub use history::*;
pub use lists::*;
pub use policy::*;
pub use summary::*;

#[cfg(test)]
mod tests;
