use crate::record_loader::{build_record_summary, format_local_time, policy_outcome};
use aura_agent_kernel::RecordObserver;
use aura_core_types::{AgentId, PolicyDecision, RecordEntry, Transaction, TransactionType};
use aura_terminal::UiCommand;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::mpsc;

/// Create a response transaction for the assistant's message.
pub(crate) fn create_response_transaction(agent_id: AgentId, response_text: &str) -> Transaction {
    Transaction::new_chained(
        agent_id,
        TransactionType::AgentMsg,
        response_text.as_bytes().to_vec(),
        None,
    )
}

/// Hands each record entry to several observers.
#[derive(Debug)]
pub struct FanoutObserver(pub Vec<Arc<dyn RecordObserver>>);

impl RecordObserver for FanoutObserver {
    fn on_entry(&self, entry: &RecordEntry) {
        for observer in &self.0 {
            observer.on_entry(entry);
        }
    }
}

/// The agent whose record the RECORD panel shows. Every agent kernel
/// shares one [`UiRecordObserver`]; entries from other agents stay in the
/// store until the panel switches to them.
#[derive(Debug, Clone, Default)]
pub struct RecordPanelAgent(Arc<Mutex<Option<AgentId>>>);

impl RecordPanelAgent {
    pub fn set(&self, agent_id: AgentId) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(agent_id);
    }

    fn shows(&self, agent_id: AgentId) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_none_or(|shown| shown == agent_id)
    }
}

/// Streams every record entry the kernel persists to the RECORD panel, in
/// sequence order, and the policy rule behind every decision to the tool
/// call it decided.
#[derive(Debug)]
pub struct UiRecordObserver {
    queue: mpsc::UnboundedSender<UiCommand>,
    panel: RecordPanelAgent,
}

impl UiRecordObserver {
    /// Must be called inside a Tokio runtime: rows are forwarded to
    /// `commands` by a task, so the kernel never blocks on a full channel
    /// and rows never overtake each other.
    #[must_use]
    pub fn new(commands: mpsc::Sender<UiCommand>, panel: RecordPanelAgent) -> Self {
        let (queue, mut rows) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(command) = rows.recv().await {
                if commands.send(command).await.is_err() {
                    break;
                }
            }
        });
        Self { queue, panel }
    }

    fn send(&self, command: UiCommand) {
        let _ = self.queue.send(command);
    }
}

impl RecordObserver for UiRecordObserver {
    fn on_entry(&self, entry: &RecordEntry) {
        if !self.panel.shows(entry.tx.agent_id) {
            return;
        }
        let timestamp = format_local_time(entry.tx.ts_ms);
        self.send(UiCommand::NewRecord(build_record_summary(
            entry.seq, &entry.tx, entry, timestamp,
        )));
        if entry.tx.tx_type == TransactionType::PolicyDecision {
            let Ok(decision) = serde_json::from_slice::<PolicyDecision>(&entry.tx.payload) else {
                return;
            };
            if let Some(outcome) = policy_outcome(&decision) {
                self.send(UiCommand::ToolPolicy {
                    id: decision.tool_use_id,
                    tool: decision.tool,
                    decision: outcome,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_types::RecordEntry;

    fn prompt(agent_id: AgentId, seq: u64) -> RecordEntry {
        RecordEntry::builder(seq, Transaction::user_prompt(agent_id, "hi")).build()
    }

    async fn seqs(rx: &mut mpsc::Receiver<UiCommand>, n: usize) -> Vec<u64> {
        let mut out = Vec::new();
        while out.len() < n {
            if let Some(UiCommand::NewRecord(row)) = rx.recv().await {
                out.push(row.seq);
            }
        }
        out
    }

    #[tokio::test]
    async fn rows_arrive_in_order_even_when_the_channel_is_full() {
        let (tx, mut rx) = mpsc::channel(1);
        let observer = UiRecordObserver::new(tx, RecordPanelAgent::default());
        let agent = AgentId::generate();
        for seq in 1..=20 {
            observer.on_entry(&prompt(agent, seq));
        }
        assert_eq!(seqs(&mut rx, 20).await, (1..=20).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn rows_from_agents_the_panel_does_not_show_are_dropped() {
        let (tx, mut rx) = mpsc::channel(8);
        let panel = RecordPanelAgent::default();
        let shown = AgentId::generate();
        panel.set(shown);
        let observer = UiRecordObserver::new(tx, panel);
        observer.on_entry(&prompt(AgentId::generate(), 1));
        observer.on_entry(&prompt(shown, 7));
        assert_eq!(seqs(&mut rx, 1).await, vec![7]);
        assert!(rx.try_recv().is_err());
    }
}
