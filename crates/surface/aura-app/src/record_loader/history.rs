//! Loading record entries and the model history they replay.

use super::*;

/// Load the most recent record entries for an agent as UI summaries,
/// oldest first.
///
/// # Errors
/// Returns the store error if the head sequence or scan fails.
pub fn recent_record_summaries(
    store: &dyn ReadStore,
    agent_id: AgentId,
) -> Result<Vec<RecordSummary>, StoreError> {
    Ok(scan_tail(store, agent_id, RECORD_PANEL_LIMIT)?
        .iter()
        .map(|entry| {
            build_record_summary(
                entry.seq,
                &entry.tx,
                entry,
                format_local_time(entry.tx.ts_ms),
            )
        })
        .collect())
}

/// Load existing records from the store and send to UI.
///
/// Uses `try_send`, so it is only suitable before the UI starts draining
/// the channel (startup); async callers should send summaries themselves.
/// The history goes as one [`UiCommand::SetRecords`], so its length does
/// not count against the channel's capacity.
pub fn load_existing_records(
    store: &dyn ReadStore,
    agent_id: AgentId,
    commands: &mpsc::Sender<UiCommand>,
) {
    match recent_record_summaries(store, agent_id) {
        Ok(summaries) => {
            if let Err(e) = commands.try_send(UiCommand::SetRecords(summaries)) {
                warn!(error = %e, "Failed to queue record history for the UI");
            }
        }
        Err(e) => {
            warn!(error = %e, "Failed to load records");
            let _ = commands.try_send(UiCommand::ShowWarning(format!(
                "Could not load record history: {e}"
            )));
        }
    }
}

/// The newest entries of an agent's record: as many as rebuilding its
/// chat, its model conversation and its plan path reads. Opening an agent
/// needs all three, so it reads them once.
#[derive(Debug, Default)]
pub struct RecordTail {
    /// The record's head when it was read
    head: u64,
    /// Oldest first
    entries: Vec<RecordEntry>,
}

impl RecordTail {
    /// The newest entries of `agent_id`'s record.
    ///
    /// # Errors
    /// Returns the store error if the head sequence or scan fails.
    pub fn read(store: &dyn ReadStore, agent_id: AgentId) -> Result<Self, StoreError> {
        let head = store.get_head_seq(agent_id)?;
        let entries = scan_below(store, agent_id, head, HISTORY_SCAN_LIMIT)?;
        Ok(Self { head, entries })
    }

    /// The record's head when it was read.
    #[must_use]
    pub const fn head(&self) -> u64 {
        self.head
    }

    /// The entries, oldest first.
    #[must_use]
    pub fn entries(&self) -> &[RecordEntry] {
        &self.entries
    }

    /// [`model_history_since`], from these entries.
    #[must_use]
    pub fn model_history_since(&self, after_seq: u64) -> Option<Vec<Message>> {
        entries_past(self.head, after_seq)?;
        messages_after(
            self.entries
                .iter()
                .filter(|entry| entry.seq > after_seq && entry.seq <= self.head),
        )
    }
}

/// Rebuild the conversation the model sees from an agent's record: its
/// prompts and replies since the last session start, oldest first. Tool
/// calls are left out; their `tool_use`/`tool_result` pairs can't be
/// replayed safely, and the replies already describe what they did.
///
/// # Errors
/// Returns the store error if the head sequence or scan fails.
pub fn model_history(store: &dyn ReadStore, agent_id: AgentId) -> Result<Vec<Message>, StoreError> {
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
pub fn model_history_since(
    store: &dyn ReadStore,
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
pub(super) fn entries_past(head_seq: u64, after_seq: u64) -> Option<usize> {
    let newer = usize::try_from(head_seq.checked_sub(after_seq)?).unwrap_or(usize::MAX);
    (newer <= HISTORY_SCAN_LIMIT).then_some(newer)
}

/// The prompts and replies in `entries`, recorded after a checkpoint;
/// `None` when a session started among them.
pub(super) fn messages_after<'a>(
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

/// Add what `tx` contributes to the model's view of the conversation.
pub(super) fn push_model_message(messages: &mut Vec<Message>, tx: &Transaction) {
    let text = || String::from_utf8_lossy(&tx.payload).trim().to_string();
    match tx.tx_type {
        TransactionType::SessionStart => messages.clear(),
        TransactionType::UserPrompt => {
            let text = prompt_text(tx).trim().to_string();
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
