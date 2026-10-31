//! Streams recorded tool policy decisions to the session's clients.

use crate::protocol::{tool_policy_decision_from_entry, OutboundMessage};
use aura_agent_kernel::ToolAuditObserver;
use aura_core_types::RecordEntry;
use tokio::sync::mpsc::{self, error::TrySendError};

/// Kernel observer that emits a `tool_policy_decision` frame for every
/// `PolicyDecision` entry the kernel persists.
#[derive(Debug)]
pub(crate) struct ToolPolicyNotifier {
    outbound: mpsc::Sender<OutboundMessage>,
}

impl ToolPolicyNotifier {
    pub(crate) const fn new(outbound: mpsc::Sender<OutboundMessage>) -> Self {
        Self { outbound }
    }
}

impl ToolAuditObserver for ToolPolicyNotifier {
    fn on_tool_entry(&self, entry: &RecordEntry) {
        let Some(decision) = tool_policy_decision_from_entry(entry) else {
            return;
        };
        match self
            .outbound
            .try_send(OutboundMessage::ToolPolicyDecision(decision))
        {
            Ok(()) | Err(TrySendError::Closed(_)) => {}
            Err(TrySendError::Full(message)) => {
                // The observer runs on the kernel task and must not block;
                // hand the frame to a task that waits for capacity.
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    let outbound = self.outbound.clone();
                    handle.spawn(async move {
                        let _ = outbound.send(message).await;
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ToolPolicyNotifier;
    use crate::protocol::OutboundMessage;
    use aura_agent_kernel::ToolAuditObserver;
    use aura_core_types::{
        AgentId, Hash, PolicyDecision, PolicyRule, PolicySource, RecordEntry, ToolGateVerdict,
        Transaction,
    };
    use tokio::sync::mpsc;

    fn decision_entry(agent_id: AgentId) -> RecordEntry {
        let decision = PolicyDecision {
            tool_use_id: "tu-1".into(),
            tool: "read_file".into(),
            verdict: ToolGateVerdict::Approved,
            reason: None,
            decided_by: PolicyRule::new(PolicySource::UserDefault, "full_access"),
            trace: Vec::new(),
            approval: None,
            mode: "agent".into(),
            policy_hash: "h".into(),
            action_id: None,
        };
        let tx =
            Transaction::policy_decision(agent_id, &decision, Hash::from_content(b"p")).unwrap();
        RecordEntry::builder(2, tx).build()
    }

    #[tokio::test]
    async fn emits_only_policy_decision_entries() {
        let (outbound_tx, mut outbound_rx) = mpsc::channel(4);
        let notifier = ToolPolicyNotifier::new(outbound_tx);
        let agent_id = AgentId::generate();

        notifier.on_tool_entry(
            &RecordEntry::builder(1, Transaction::user_prompt(agent_id, "hi")).build(),
        );
        notifier.on_tool_entry(&decision_entry(agent_id));

        match outbound_rx.recv().await.expect("decision frame") {
            OutboundMessage::ToolPolicyDecision(frame) => {
                assert_eq!(frame.tool_name, "read_file");
                assert_eq!(frame.decided_by.rule, "full_access");
                assert_eq!(frame.seq, 2);
            }
            other => panic!("unexpected frame: {other:?}"),
        }
        assert!(outbound_rx.try_recv().is_err());
    }
}
