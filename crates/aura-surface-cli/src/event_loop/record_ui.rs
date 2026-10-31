use crate::record_loader::build_record_summary;
use aura_core_types::{AgentId, RecordEntry, Transaction, TransactionType};
use aura_terminal::UiCommand;
use tokio::sync::mpsc;

/// Send a record summary to the UI (matching the stored format).
pub(crate) async fn send_record_to_ui(
    commands: &mpsc::Sender<UiCommand>,
    seq: u64,
    tx: &Transaction,
    entry: &RecordEntry,
) {
    let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
    let record_summary = build_record_summary(seq, tx, entry, timestamp);
    let _ = commands.send(UiCommand::NewRecord(record_summary)).await;
}

/// Create a response transaction for the assistant's message.
pub(crate) fn create_response_transaction(agent_id: AgentId, response_text: &str) -> Transaction {
    Transaction::new_chained(
        agent_id,
        TransactionType::AgentMsg,
        response_text.as_bytes().to_vec(),
        None,
    )
}
