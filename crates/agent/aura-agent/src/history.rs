//! The conversation an agent's model continues from.
//!
//! Every surface that runs an agent (the terminal, a cloud machine's node)
//! resumes it the same way: the agent's current [`SessionCheckpoint`],
//! with the prompts and replies recorded after it, or, without a
//! checkpoint that still applies, the prompts and replies the record holds
//! since the last session start.

use crate::checkpoint::{is_resumable, Checkpoints, SessionCheckpoint};
use aura_core_types::{AgentId, RecordEntry, Transaction, TransactionType};
use aura_model::Message;
use aura_store_db::{ReadStore, StoreError};
use tracing::warn;

/// Number of trailing entries scanned when rebuilding an agent's
/// conversation.
pub const HISTORY_SCAN_LIMIT: usize = 1000;

/// The last `limit` entries of `agent_id`'s record, oldest first.
///
/// # Errors
/// Returns the store error if the head sequence or scan fails.
pub fn scan_tail<S: ReadStore + ?Sized>(
    store: &S,
    agent_id: AgentId,
    limit: usize,
) -> Result<Vec<RecordEntry>, StoreError> {
    let head_seq = store.get_head_seq(agent_id)?;
    scan_below(store, agent_id, head_seq, limit)
}

/// The newest `limit` entries of `agent_id`'s record up to `head_seq`,
/// oldest first.
///
/// # Errors
/// Returns the store error if the scan fails.
pub fn scan_below<S: ReadStore + ?Sized>(
    store: &S,
    agent_id: AgentId,
    head_seq: u64,
    limit: usize,
) -> Result<Vec<RecordEntry>, StoreError> {
    if head_seq == 0 {
        return Ok(Vec::new());
    }
    let window = u64::try_from(limit).unwrap_or(u64::MAX);
    let from_seq = head_seq.saturating_sub(window.saturating_sub(1)).max(1);
    store.scan_record(agent_id, from_seq, limit)
}

/// Rebuild the conversation the model sees from an agent's record: its
/// prompts and replies since the last session start, oldest first. Tool
/// calls are left out; their `tool_use`/`tool_result` pairs can't be
/// replayed safely, and the replies already describe what they did.
///
/// # Errors
/// Returns the store error if the head sequence or scan fails.
pub fn model_history<S: ReadStore + ?Sized>(
    store: &S,
    agent_id: AgentId,
) -> Result<Vec<Message>, StoreError> {
    Ok(model_history_from(&scan_tail(
        store,
        agent_id,
        HISTORY_SCAN_LIMIT,
    )?))
}

/// [`model_history`] from `entries`, the newest of the record, oldest
/// first.
#[must_use]
pub fn model_history_from(entries: &[RecordEntry]) -> Vec<Message> {
    let mut messages = Vec::new();
    for entry in entries {
        push_model_message(&mut messages, &entry.tx);
    }
    messages
}

/// The prompts and replies recorded after `after_seq`, for replaying on
/// top of a checkpoint taken at `after_seq`. `None` when the checkpoint
/// no longer applies: a session started since, or too much happened to
/// scan.
///
/// # Errors
/// Returns the store error if the head sequence or scan fails.
pub fn model_history_since<S: ReadStore + ?Sized>(
    store: &S,
    agent_id: AgentId,
    after_seq: u64,
) -> Result<Option<Vec<Message>>, StoreError> {
    let head_seq = store.get_head_seq(agent_id)?;
    let Some(newer) = entries_past(head_seq, after_seq) else {
        return Ok(None);
    };
    if newer == 0 {
        return Ok(Some(Vec::new()));
    }
    Ok(messages_after(&store.scan_record(
        agent_id,
        after_seq + 1,
        newer,
    )?))
}

/// How many entries a record with head `head_seq` holds past a checkpoint
/// taken at `after_seq`, while the checkpoint applies: it is not ahead of
/// the record, and what came after it is few enough to scan.
#[must_use]
pub fn entries_past(head_seq: u64, after_seq: u64) -> Option<usize> {
    let newer = usize::try_from(head_seq.checked_sub(after_seq)?).unwrap_or(usize::MAX);
    (newer <= HISTORY_SCAN_LIMIT).then_some(newer)
}

/// The prompts and replies in `entries`, recorded after a checkpoint;
/// `None` when a session started among them.
pub fn messages_after<'a>(
    entries: impl IntoIterator<Item = &'a RecordEntry>,
) -> Option<Vec<Message>> {
    let mut messages = Vec::new();
    for entry in entries {
        if entry.tx.tx_type == TransactionType::SessionStart {
            return None;
        }
        push_model_message(&mut messages, &entry.tx);
    }
    Some(messages)
}

/// The agent's current checkpoint with what the record holds beyond it,
/// when the checkpoint still applies. A tool call the checkpoint ends on
/// is dropped: its result was never saved.
#[must_use]
pub fn restore<S: ReadStore + ?Sized>(
    store: &S,
    checkpoints: &Checkpoints,
    agent_id: AgentId,
) -> Option<SessionCheckpoint> {
    restore_with(checkpoints, agent_id, |after_seq| {
        model_history_since(store, agent_id, after_seq).unwrap_or_else(|e| {
            warn!(error = %e, "Failed to read the record past the checkpoint");
            None
        })
    })
}

/// [`restore`], with what the record holds past the checkpoint's sequence
/// from `since` (`None` when the checkpoint no longer applies), as from
/// entries already read.
pub fn restore_with(
    checkpoints: &Checkpoints,
    agent_id: AgentId,
    since: impl FnOnce(u64) -> Option<Vec<Message>>,
) -> Option<SessionCheckpoint> {
    let mut checkpoint = checkpoints.load_current(agent_id)?;
    let newer = since(checkpoint.record_seq)?;
    if !is_resumable(&checkpoint.messages) {
        checkpoint.messages.pop();
    }
    checkpoint.messages.extend(newer);
    Some(checkpoint)
}

/// Add what `tx` contributes to the model's view of the conversation.
fn push_model_message(messages: &mut Vec<Message>, tx: &Transaction) {
    let text = || String::from_utf8_lossy(&tx.payload).trim().to_string();
    match tx.tx_type {
        TransactionType::SessionStart => messages.clear(),
        TransactionType::UserPrompt => {
            let text = tx
                .prompt_payload()
                .map(|prompt| prompt.text)
                .unwrap_or_default()
                .trim()
                .to_string();
            if !text.is_empty() {
                messages.push(Message::user(text));
            }
        }
        TransactionType::AgentMsg => {
            let text = text();
            if !text.is_empty() {
                messages.push(Message::assistant(text));
            }
        }
        _ => {}
    }
}
