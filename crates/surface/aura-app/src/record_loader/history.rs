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
