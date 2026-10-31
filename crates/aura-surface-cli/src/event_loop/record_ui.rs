use crate::record_loader::{build_record_summary, format_local_time, policy_chat_line};
use aura_agent_kernel::ToolAuditObserver;
use aura_core_types::{AgentId, PolicyDecision, RecordEntry, Transaction, TransactionType};
use aura_terminal::events::{MessageData, MessageRole};
use aura_terminal::UiCommand;
use tokio::sync::mpsc::{self, error::TrySendError};

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

/// Streams each tool-call record entry (proposal, policy decision,
/// execution) to the RECORD panel as the kernel persists it, plus a chat
/// line naming the policy rule behind every decision.
#[derive(Debug)]
pub struct UiToolAuditObserver {
    commands: mpsc::Sender<UiCommand>,
}

impl UiToolAuditObserver {
    #[must_use]
    pub const fn new(commands: mpsc::Sender<UiCommand>) -> Self {
        Self { commands }
    }

    fn send(&self, command: UiCommand) {
        if let Err(TrySendError::Full(command)) = self.commands.try_send(command) {
            // Runs on the kernel task, so never block; wait for capacity
            // on a separate task instead.
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                let commands = self.commands.clone();
                handle.spawn(async move {
                    let _ = commands.send(command).await;
                });
            }
        }
    }
}

impl ToolAuditObserver for UiToolAuditObserver {
    fn on_tool_entry(&self, entry: &RecordEntry) {
        let timestamp = format_local_time(entry.tx.ts_ms);
        self.send(UiCommand::NewRecord(build_record_summary(
            entry.seq, &entry.tx, entry, timestamp,
        )));
        if entry.tx.tx_type == TransactionType::PolicyDecision {
            let line = serde_json::from_slice::<PolicyDecision>(&entry.tx.payload)
                .ok()
                .and_then(|decision| policy_chat_line(&decision));
            if let Some(content) = line {
                self.send(UiCommand::ShowMessage(MessageData {
                    role: MessageRole::System,
                    content,
                    is_streaming: false,
                    timestamp_ms: Some(entry.tx.ts_ms),
                    command: None,
                    activity: None,
                }));
            }
        }
    }
}
