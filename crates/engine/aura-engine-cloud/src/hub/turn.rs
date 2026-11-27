//! A cloud agent's turns over its node's wire API.

use std::sync::Arc;

use crate::{BootEvent, BootReporter, CloudError, CloudResult};
use aura_config::BoxdToken;
use aura_core_protocol::{
    InboundMessage, OutboundMessage, RuntimeRequest, SessionUsage, UserMessage,
};
use aura_core_types::AgentId;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use super::node_client::NodeClient;
use super::request::{chat_request, TurnAsk};

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
    /// The chat's title, written on the machine from the turn's prompt.
    Title(String),
    /// The turn ended, with the session's usage and context so far.
    Done(Box<SessionUsage>),
    /// The turn failed.
    Failed(String),
}

impl CloudHub {
    /// Wake `hex`'s node, showing the machine waking on `tx`. A machine
    /// that sleeps shows itself waking; the events all reach the turn's
    /// stream before anything else does. A node older than this hub is
    /// refused ([`CloudHub::current_node`]).
    async fn wake_for_turn(
        &self,
        hex: &str,
        tx: &mpsc::Sender<RemoteEvent>,
    ) -> CloudResult<NodeClient> {
        let (boot, mut events) = BootReporter::channel();
        let shown = tx.clone();
        let forward = tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                if shown.send(RemoteEvent::Boot(event)).await.is_err() {
                    break;
                }
            }
        });
        let awake = self.current_node(hex, &boot).await;
        drop(boot);
        let _ = forward.await;
        awake
    }

    /// Run `ask` on `agent`'s node and stream what it shows. The node
    /// writes the turn; the hub mirrors it as it runs and when it ends.
    /// With a title model, the node also titles the chat from the prompt,
    /// sent as [`RemoteEvent::Title`].
    #[must_use]
    pub fn run_turn(
        self: &Arc<Self>,
        agent: AgentId,
        ask: TurnAsk,
        cancel: CancellationToken,
    ) -> mpsc::Receiver<RemoteEvent> {
        let (tx, rx) = mpsc::channel(256);
        let hub = Arc::clone(self);
        tokio::spawn(async move {
            let hex = agent.to_hex();
            let _ = tx
                .send(RemoteEvent::Status("Waking cloud machine".into()))
                .await;
            let node = match hub.wake_for_turn(&hex, &tx).await {
                Ok(node) => node,
                Err(err) => {
                    let _ = tx.send(RemoteEvent::Failed(err.to_string())).await;
                    return;
                }
            };
            let guard = hub.fleet.begin_work(&hex);
            let request = chat_request(agent, &ask);
            let turn = tokio::time::timeout(
                TURN_TIMEOUT,
                stream_turn(
                    &hub,
                    &node,
                    TurnPrompt {
                        agent,
                        prompt: ask.prompt,
                        request,
                    },
                    &tx,
                    cancel,
                ),
            )
            .await
            .unwrap_or(Err(CloudError::Timeout {
                op: "turn",
                after: TURN_TIMEOUT,
            }));
            if let Err(err) = hub.mirror_from(&node, agent).await {
                tracing::warn!(agent = %hex, error = %err, "could not copy the turn's record");
            }
            // The machine stays awake for these: the guard is still held.
            if let Err(err) = hub.copy_session(&node, agent).await {
                tracing::debug!(agent = %hex, error = %err, "could not copy the session checkpoint");
            }
            if let Err(err) = hub.copy_traces(&node, agent).await {
                tracing::debug!(agent = %hex, error = %err, "could not copy the trace runs");
            }
            drop(guard);
            if let Err(err) = turn {
                let _ = tx.send(RemoteEvent::Failed(err.to_string())).await;
            }
        });
        rx
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

/// The mesh or event socket to a node.
pub(super) type NodeSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
/// The sending half of a [`NodeSocket`].
pub(super) type NodeSink = futures_util::stream::SplitSink<NodeSocket, Message>;
/// The receiving half of a [`NodeSocket`].
pub(super) type NodeStream = futures_util::stream::SplitStream<NodeSocket>;

/// One prompt for a cloud agent's node, with the chat run that carries it.
struct TurnPrompt {
    agent: AgentId,
    prompt: String,
    request: RuntimeRequest,
}

/// One turn: start the chat run `turn.request` asks for, send the prompt,
/// map the stream.
async fn stream_turn(
    hub: &CloudHub,
    node: &NodeClient,
    turn: TurnPrompt,
    tx: &mpsc::Sender<RemoteEvent>,
    cancel: CancellationToken,
) -> CloudResult<()> {
    let started = node.start_run(&turn.request).await?;
    let _streamed = started
        .get("run_id")
        .and_then(serde_json::Value::as_str)
        .map(|run| hub.streaming_turn(&turn.agent.to_hex(), run.to_string()));
    let path = stream_path(&started)?;
    let url = ws_url(node.base(), &path);
    let (socket, _) = tokio_tungstenite::connect_async(ws_request(&url, node.token())?)
        .await
        .map_err(|err| CloudError::Transport {
            endpoint: url.clone(),
            message: err.to_string(),
        })?;
    let (mut sink, mut stream) = socket.split();
    let message = InboundMessage::UserMessage(UserMessage {
        content: turn.prompt,
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
    TurnStream {
        hub,
        node,
        agent: turn.agent,
        url,
        sink: &mut sink,
        stream: &mut stream,
        tx,
    }
    .relay(cancel)
    .await
}

/// Where a started run's events stream from.
fn stream_path(started: &serde_json::Value) -> CloudResult<String> {
    started
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
        })
}

/// A turn's open event stream.
struct TurnStream<'a> {
    hub: &'a CloudHub,
    node: &'a NodeClient,
    agent: AgentId,
    url: String,
    sink: &'a mut NodeSink,
    stream: &'a mut NodeStream,
    tx: &'a mpsc::Sender<RemoteEvent>,
}

impl TurnStream<'_> {
    /// Show the stream's events until the turn ends, mirroring the
    /// record as it runs.
    async fn relay(mut self, cancel: CancellationToken) -> CloudResult<()> {
        let mut mirror = tokio::time::interval(MIRROR_EVERY);
        mirror.tick().await;
        loop {
            tokio::select! {
                () = cancel.cancelled() => {
                    let cancel = serde_json::to_string(&InboundMessage::Cancel).unwrap_or_default();
                    let _ = self.sink.send(Message::Text(cancel.into())).await;
                    return Err(CloudError::BadState { machine: self.node.node().into(), state: "cancelled".into(), op: "finish the turn" });
                }
                _ = mirror.tick() => {
                    if let Err(err) = self.hub.mirror_from(self.node, self.agent).await {
                        tracing::debug!(error = %err, "mirror during a turn failed");
                    }
                }
                frame = self.stream.next() => {
                    let Some(frame) = frame else {
                        return Err(CloudError::Node { node: self.node.node().into(), op: "stream", message: "closed before the turn ended".into() });
                    };
                    let frame = frame.map_err(|err| CloudError::Transport { endpoint: self.url.clone(), message: err.to_string() })?;
                    if let Some(end) = self.take_frame(frame).await {
                        return end;
                    }
                }
            }
        }
    }

    /// Show one frame; `Some` when the turn is over.
    async fn take_frame(&mut self, frame: Message) -> Option<CloudResult<()>> {
        let Message::Text(text) = frame else {
            return None;
        };
        let Ok(message) = serde_json::from_str::<OutboundMessage>(&text) else {
            return None;
        };
        match map_outbound(message) {
            Some(RemoteEvent::Failed(err)) => Some(Err(CloudError::Node {
                node: self.node.node().into(),
                op: "turn",
                message: err,
            })),
            Some(event @ RemoteEvent::Done { .. }) => {
                let _ = self.tx.send(event).await;
                Some(Ok(()))
            }
            Some(event) => self.tx.send(event).await.is_err().then_some(Ok(())),
            None => None,
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
        OutboundMessage::AssistantMessageEnd(end) => RemoteEvent::Done(Box::new(end.usage)),
        OutboundMessage::ChatTitle(title) => RemoteEvent::Title(title.title),
        OutboundMessage::Error(err) if !err.recoverable => RemoteEvent::Failed(err.message),
        _ => return None,
    })
}
