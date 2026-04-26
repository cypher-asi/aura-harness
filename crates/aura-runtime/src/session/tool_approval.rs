//! Live WebSocket approval broker for tri-state `ask` tool calls.

use crate::protocol::{
    OutboundMessage, ToolApprovalDecision as ProtocolDecision,
    ToolApprovalPrompt as ProtocolPrompt, ToolApprovalRemember as ProtocolRemember,
    ToolApprovalResponse as ProtocolResponse,
};
use async_trait::async_trait;
use aura_core::ToolState;
use aura_kernel::{
    PendingToolPrompt, ToolApprovalError, ToolApprovalPrompter, ToolApprovalRemember,
    ToolApprovalResponse,
};
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{timeout, Duration};

const TOOL_APPROVAL_DELIVERY_TIMEOUT: Duration = Duration::from_millis(500);

/// Per-connection prompt registry keyed by `request_id`.
#[derive(Debug)]
pub(crate) struct ToolApprovalBroker {
    outbound: mpsc::Sender<OutboundMessage>,
    pending: Mutex<HashMap<String, oneshot::Sender<ToolApprovalResponse>>>,
}

impl ToolApprovalBroker {
    pub(crate) fn new(outbound: mpsc::Sender<OutboundMessage>) -> Self {
        Self {
            outbound,
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn respond(&self, response: ProtocolResponse) -> Result<(), String> {
        let request_id = response.request_id.clone();
        let response = ToolApprovalResponse {
            decision: match response.decision {
                ProtocolDecision::On => ToolState::Allow,
                ProtocolDecision::Off => ToolState::Deny,
            },
            remember: match response.remember {
                ProtocolRemember::Once => ToolApprovalRemember::Once,
                ProtocolRemember::Session => ToolApprovalRemember::Session,
                ProtocolRemember::Forever => ToolApprovalRemember::Forever,
            },
        };

        let sender = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&request_id)
            .ok_or_else(|| format!("No pending tool approval for request_id '{request_id}'"))?;
        sender
            .send(response)
            .map_err(|_| format!("Tool approval request '{request_id}' is no longer active"))
    }
}

#[async_trait]
impl ToolApprovalPrompter for ToolApprovalBroker {
    async fn prompt(
        &self,
        prompt: PendingToolPrompt,
    ) -> Result<ToolApprovalResponse, ToolApprovalError> {
        let request_id = prompt.request_id.clone();
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(request_id.clone(), tx);

        let outbound = OutboundMessage::ToolApprovalPrompt(ProtocolPrompt {
            request_id: request_id.clone(),
            tool_name: prompt.tool_name,
            args: prompt.args,
            agent_id: prompt.agent_id.to_hex(),
            remember_options: prompt
                .remember_options
                .into_iter()
                .map(|remember| match remember {
                    ToolApprovalRemember::Once => ProtocolRemember::Once,
                    ToolApprovalRemember::Session => ProtocolRemember::Session,
                    ToolApprovalRemember::Forever => ProtocolRemember::Forever,
                })
                .collect(),
        });

        if !matches!(
            timeout(TOOL_APPROVAL_DELIVERY_TIMEOUT, self.outbound.send(outbound)).await,
            Ok(Ok(()))
        ) {
            self.pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&request_id);
            return Err(ToolApprovalError::DeliveryFailed);
        }

        rx.await.map_err(|_| ToolApprovalError::Cancelled)
    }
}

#[cfg(test)]
mod tests {
    use super::ToolApprovalBroker;
    use crate::protocol::{
        OutboundMessage, TextDelta, ToolApprovalDecision, ToolApprovalRemember,
        ToolApprovalResponse as ProtocolResponse,
    };
    use aura_core::{AgentId, ToolState};
    use aura_kernel::{
        PendingToolPrompt, ToolApprovalError, ToolApprovalPrompter,
        ToolApprovalRemember as Remember,
    };
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use tokio::time::Duration;

    fn prompt(request_id: &str) -> PendingToolPrompt {
        PendingToolPrompt {
            request_id: request_id.to_string(),
            tool_name: "run_command".to_string(),
            args: serde_json::json!({ "cmd": "echo ok" }),
            agent_id: AgentId::new([1; 32]),
            remember_options: vec![Remember::Once],
        }
    }

    #[tokio::test]
    async fn prompt_waits_for_outbound_capacity() {
        let (outbound_tx, mut outbound_rx) = mpsc::channel(1);
        outbound_tx
            .send(OutboundMessage::TextDelta(TextDelta {
                text: "backlog".to_string(),
            }))
            .await
            .expect("fill outbound channel");

        let broker = Arc::new(ToolApprovalBroker::new(outbound_tx));
        let broker_for_prompt = Arc::clone(&broker);
        let prompt_task =
            tokio::spawn(async move { broker_for_prompt.prompt(prompt("req-1")).await });

        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(
            !prompt_task.is_finished(),
            "prompt should backpressure instead of dropping while channel is full"
        );

        let _ = outbound_rx.recv().await.expect("drain backlog");
        let outbound = outbound_rx
            .recv()
            .await
            .expect("approval prompt should be delivered after capacity opens");
        assert!(matches!(
            outbound,
            OutboundMessage::ToolApprovalPrompt(ref prompt) if prompt.request_id == "req-1"
        ));

        broker
            .respond(ProtocolResponse {
                request_id: "req-1".to_string(),
                decision: ToolApprovalDecision::On,
                remember: ToolApprovalRemember::Once,
            })
            .expect("respond to prompt");

        let response = prompt_task
            .await
            .expect("prompt task joins")
            .expect("approval");
        assert_eq!(response.decision, ToolState::Allow);
    }

    #[tokio::test]
    async fn prompt_cleans_pending_on_delivery_failure() {
        let (outbound_tx, outbound_rx) = mpsc::channel(1);
        drop(outbound_rx);
        let broker = ToolApprovalBroker::new(outbound_tx);

        let result = broker.prompt(prompt("req-2")).await;

        assert!(matches!(result, Err(ToolApprovalError::DeliveryFailed)));
        assert!(broker
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty());
    }
}
