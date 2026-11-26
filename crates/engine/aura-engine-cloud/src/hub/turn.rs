//! A cloud agent's turns over its node's wire API.

use std::sync::Arc;

use crate::machine_env::WORKSPACE_DIR;
use crate::{BootEvent, BootReporter, CloudError, CloudResult};
use aura_config::BoxdToken;
use aura_core_protocol::{
    AgentCapabilities, AgentIdentity, AgentPermissionsWire, InboundMessage, ModelSelection,
    OutboundMessage, RuntimeRequest, RuntimeRequestType, UserMessage, WorkspaceLocation,
};
use aura_core_types::AgentId;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::node_client::NodeClient;

use super::{CloudHub, MIRROR_EVERY, TURN_TIMEOUT};

/// What a cloud agent's turn shows, as it streams.
#[derive(Debug, Clone, PartialEq)]
pub enum RemoteEvent {
    /// The machine is waking or the run is starting.
    Status(String),
    /// A step of waking a sleeping machine.
    Boot(BootEvent),
    /// Reply text.
    Text(String),
    /// Thinking text.
    Thinking(String),
    /// A tool call started.
    ToolStart {
        /// Tool use id.
        id: String,
        /// Tool name.
        name: String,
    },
    /// A tool call's input so far.
    ToolInput {
        /// Tool use id.
        id: String,
        /// Input JSON.
        input: String,
    },
    /// A tool call finished.
    ToolResult {
        /// Tool use id.
        id: String,
        /// Result text.
        result: String,
        /// Whether it failed.
        is_error: bool,
    },
    /// The turn ended.
    Done {
        /// Model used.
        model: String,
        /// Input tokens of the turn.
        input_tokens: u64,
        /// Output tokens of the turn.
        output_tokens: u64,
    },
    /// The turn failed.
    Failed(String),
}

impl CloudHub {
    /// Run one prompt on `agent`'s node and stream what it shows. The
    /// node writes the turn; the hub mirrors it as it runs and when it
    /// ends.
    #[must_use]
    pub fn run_turn(
        self: &Arc<Self>,
        agent: AgentId,
        prompt: String,
        model: Option<String>,
        auth_jwt: Option<String>,
        cancel: CancellationToken,
    ) -> mpsc::Receiver<RemoteEvent> {
        let (tx, rx) = mpsc::channel(256);
        let hub = Arc::clone(self);
        tokio::spawn(async move {
            let hex = agent.to_hex();
            let _ = tx
                .send(RemoteEvent::Status("Waking cloud machine".into()))
                .await;
            // A machine that sleeps shows itself waking; the events all
            // reach the turn's stream before anything else does.
            let (boot, mut events) = BootReporter::channel();
            let shown = tx.clone();
            let forward = tokio::spawn(async move {
                while let Some(event) = events.recv().await {
                    if shown.send(RemoteEvent::Boot(event)).await.is_err() {
                        break;
                    }
                }
            });
            let awake = hub.awake_node(&hex, &boot).await;
            drop(boot);
            let _ = forward.await;
            let node = match awake {
                Ok(node) => node,
                Err(err) => {
                    let _ = tx.send(RemoteEvent::Failed(err.to_string())).await;
                    return;
                }
            };
            let guard = hub.fleet.begin_work(&hex);
            let turn = tokio::time::timeout(
                TURN_TIMEOUT,
                stream_turn(&hub, &node, agent, prompt, model, auth_jwt, &tx, cancel),
            )
            .await
            .unwrap_or(Err(CloudError::Timeout {
                op: "turn",
                after: TURN_TIMEOUT,
            }));
            if let Err(err) = hub.mirror_from(&node, agent).await {
                tracing::warn!(agent = %hex, error = %err, "could not copy the turn's record");
            }
            drop(guard);
            if let Err(err) = turn {
                let _ = tx.send(RemoteEvent::Failed(err.to_string())).await;
            }
        });
        rx
    }
}

/// The chat run request for a cloud agent's turn.
pub(super) fn chat_request(
    agent: AgentId,
    model: Option<String>,
    auth_jwt: Option<String>,
) -> RuntimeRequest {
    RuntimeRequest {
        r#type: RuntimeRequestType::Chat {
            conversation_messages: Vec::new(),
        },
        agent_identity: AgentIdentity {
            partition_id: Some(agent.to_hex()),
            ..AgentIdentity::default()
        },
        model: ModelSelection {
            id: model,
            ..ModelSelection::default()
        },
        workspace: WorkspaceLocation {
            project_path: Some(WORKSPACE_DIR.to_string()),
            ..WorkspaceLocation::default()
        },
        project: None,
        agent_permissions: cloud_agent_permissions(),
        tool_permissions: None,
        agent_capabilities: AgentCapabilities::default(),
        auth_jwt,
        user_id: "hub".to_string(),
        mode: None,
    }
}

/// What a cloud agent may do on its machine. Sent explicitly: an empty
/// bundle means full access on a node, which includes computer use, and
/// a cloud machine has no screen (and OpenRouter refuses the computer
/// tool's server-tool entry).
fn cloud_agent_permissions() -> AgentPermissionsWire {
    use aura_core_protocol::CapabilityWire;
    AgentPermissionsWire {
        capabilities: vec![
            CapabilityWire::SpawnAgent,
            CapabilityWire::ControlAgent,
            CapabilityWire::ReadAgent,
            CapabilityWire::ListAgents,
            CapabilityWire::InvokeProcess,
            CapabilityWire::ReadAllProjects,
            CapabilityWire::WriteAllProjects,
        ],
        ..AgentPermissionsWire::default()
    }
}

/// `https://x` → `wss://x`.
pub(super) fn ws_url(base: &str, path: &str) -> String {
    let base = base
        .strip_prefix("https://")
        .map(|rest| format!("wss://{rest}"))
        .or_else(|| {
            base.strip_prefix("http://")
                .map(|rest| format!("ws://{rest}"))
        })
        .unwrap_or_else(|| base.to_string());
    format!("{base}{path}")
}

pub(super) fn ws_request(
    url: &str,
    token: &BoxdToken,
) -> CloudResult<tokio_tungstenite::tungstenite::handshake::client::Request> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = url
        .into_client_request()
        .map_err(|err| CloudError::Transport {
            endpoint: url.to_string(),
            message: err.to_string(),
        })?;
    let value = format!("Bearer {}", token.expose())
        .parse()
        .map_err(|_| CloudError::Auth("node token is not a valid header".into()))?;
    request.headers_mut().insert("authorization", value);
    Ok(request)
}

/// One turn: start the chat run, send the prompt, map the stream.
#[allow(clippy::too_many_arguments)]
async fn stream_turn(
    hub: &CloudHub,
    node: &NodeClient,
    agent: AgentId,
    prompt: String,
    model: Option<String>,
    auth_jwt: Option<String>,
    tx: &mpsc::Sender<RemoteEvent>,
    cancel: CancellationToken,
) -> CloudResult<()> {
    use tokio_tungstenite::tungstenite::Message;
    let started = node
        .start_run(&chat_request(agent, model, auth_jwt))
        .await?;
    let path = started
        .get("event_stream_url")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            started
                .get("run_id")
                .and_then(serde_json::Value::as_str)
                .map(|id| format!("/stream/{id}"))
        })
        .ok_or_else(|| CloudError::InvalidResponse {
            op: "run",
            message: "no run id".into(),
        })?;
    let url = ws_url(node.base(), &path);
    let (socket, _) = tokio_tungstenite::connect_async(ws_request(&url, node.token())?)
        .await
        .map_err(|err| CloudError::Transport {
            endpoint: url.clone(),
            message: err.to_string(),
        })?;
    let (mut sink, mut stream) = socket.split();
    let message = InboundMessage::UserMessage(UserMessage {
        content: prompt,
        tool_hints: None,
        attachments: None,
    });
    let text = serde_json::to_string(&message).unwrap_or_default();
    sink.send(Message::Text(text.into()))
        .await
        .map_err(|err| CloudError::Transport {
            endpoint: url.clone(),
            message: err.to_string(),
        })?;
    let _ = tx
        .send(RemoteEvent::Status("Working in the cloud".into()))
        .await;
    let mut mirror = tokio::time::interval(MIRROR_EVERY);
    mirror.tick().await;
    loop {
        tokio::select! {
            () = cancel.cancelled() => {
                let cancel = serde_json::to_string(&InboundMessage::Cancel).unwrap_or_default();
                let _ = sink.send(Message::Text(cancel.into())).await;
                return Err(CloudError::BadState { machine: node.node().into(), state: "cancelled".into(), op: "finish the turn" });
            }
            _ = mirror.tick() => {
                if let Err(err) = hub.mirror_from(node, agent).await {
                    tracing::debug!(error = %err, "mirror during a turn failed");
                }
            }
            frame = stream.next() => {
                let Some(frame) = frame else {
                    return Err(CloudError::Node { node: node.node().into(), op: "stream", message: "closed before the turn ended".into() });
                };
                let frame = frame.map_err(|err| CloudError::Transport { endpoint: url.clone(), message: err.to_string() })?;
                let Message::Text(text) = frame else { continue };
                let Ok(message) = serde_json::from_str::<OutboundMessage>(&text) else { continue };
                match map_outbound(message) {
                    Some(RemoteEvent::Failed(err)) => return Err(CloudError::Node { node: node.node().into(), op: "turn", message: err }),
                    Some(event @ RemoteEvent::Done { .. }) => {
                        let _ = tx.send(event).await;
                        return Ok(());
                    }
                    Some(event) => {
                        if tx.send(event).await.is_err() {
                            return Ok(());
                        }
                    }
                    None => {}
                }
            }
        }
    }
}

/// What a node's stream frame shows, if anything.
#[must_use]
pub fn map_outbound(message: OutboundMessage) -> Option<RemoteEvent> {
    Some(match message {
        OutboundMessage::TextDelta(delta) => RemoteEvent::Text(delta.text),
        OutboundMessage::ThinkingDelta(delta) => RemoteEvent::Thinking(delta.thinking),
        OutboundMessage::ToolUseStart(start) => RemoteEvent::ToolStart {
            id: start.id,
            name: start.name,
        },
        OutboundMessage::ToolCallSnapshot(snapshot) => RemoteEvent::ToolInput {
            id: snapshot.id,
            input: snapshot.input.to_string(),
        },
        OutboundMessage::ToolResult(result) => RemoteEvent::ToolResult {
            id: result.tool_use_id.unwrap_or_default(),
            result: result.result,
            is_error: result.is_error,
        },
        OutboundMessage::AssistantMessageEnd(end) => RemoteEvent::Done {
            model: end.usage.model.clone(),
            input_tokens: end.usage.input_tokens,
            output_tokens: end.usage.output_tokens,
        },
        OutboundMessage::Error(err) if !err.recoverable => RemoteEvent::Failed(err.message),
        _ => return None,
    })
}
