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

        if self.outbound.try_send(outbound).is_err() {
            self.pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&request_id);
            return Err(ToolApprovalError::DeliveryFailed);
        }

        rx.await.map_err(|_| ToolApprovalError::Cancelled)
    }
}
