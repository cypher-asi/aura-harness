//! A cloud agent's turns over its node's wire API.

use std::sync::Arc;

use crate::fleet::WorkGuard;
use crate::{BootEvent, BootReporter, CloudError, CloudResult};
use aura_config::BoxdToken;
use aura_core_protocol::{
    InboundMessage, OutboundMessage, RuntimeRequest, SessionUsage, UserMessage,
};
use aura_core_types::AgentId;
use futures_util::SinkExt;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use super::follow::{RunWatch, Shown, StreamEnd};
use super::marks::TurnMark;
use super::node_client::{NodeClient, NODE_TIMEOUT};
use super::request::{chat_request, TurnAsk};

use super::{CloudHub, TURN_TIMEOUT};

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
    /// Output a tool call printed while it is still running.
    ToolOutput {
        /// Tool use id.
        id: String,
        /// The next piece of output.
        chunk: String,
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
    /// A model call of the turn ended. Reply text after it is a new reply.
    ModelCallEnd,
    /// Entries the turn wrote were copied here. The stream of a turn
    /// attached to again, or followed by a hub started since it was
    /// asked for, shows no more than how it ends: its chat shows the
    /// turn from the record instead.
    Copied,
    /// The turn ended, with the session's usage and context so far
    /// (empty for a turn that ended while no hub watched).
    Done(Box<SessionUsage>),
    /// The turn failed.
    Failed(String),
}

impl CloudHub {
    /// Wake `hex`'s node, showing the machine waking on `tx`, and keep a
    /// socket to it open. A machine that sleeps shows itself waking; the
    /// events all reach the turn's stream before anything else does. A
    /// node older than this hub is refused ([`CloudHub::current_node`]).
    pub(super) async fn wake_for_turn(
        self: &Arc<Self>,
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
        if awake.is_ok() {
            self.ensure_link(hex);
        }
        awake
    }

    /// Run `ask` on `agent`'s node and stream what it shows. The node
    /// writes the turn; the hub mirrors it as it runs and when it ends.
    /// With a title model, the node also titles the chat from the prompt,
    /// sent as [`RemoteEvent::Title`].
    ///
    /// The turn is marked on the agent's placement until its end is
    /// seen, and runs on when the hub stops or crashes: a hub started
    /// since follows it ([`Self::follow_turn`]). A stream that drops is
    /// attached to again, quietly ([`RemoteEvent::Copied`]).
    #[must_use]
    pub fn run_turn(
        self: &Arc<Self>,
        agent: AgentId,
        mut ask: TurnAsk,
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
            hub.deliver_skills(&node, agent, &mut ask).await;
            let guard = hub.fleet.begin_work(&hex);
            let turn = TurnPrompt {
                agent,
                request: chat_request(agent, &ask),
                prompt: ask.prompt,
            };
            let (run_id, end) = hub.drive_turn(&node, &turn, &tx, &cancel).await;
            hub.finish_turn(&node, agent, run_id.as_deref(), end, guard, &tx)
                .await;
        });
        rx
    }

    /// One turn: start its chat run, send the prompt and mark the turn,
    /// then follow the run until the turn ends. Returns the run once it
    /// started, and how the turn ended.
    async fn drive_turn(
        &self,
        node: &NodeClient,
        turn: &TurnPrompt,
        tx: &mpsc::Sender<RemoteEvent>,
        cancel: &CancellationToken,
    ) -> (Option<String>, StreamEnd) {
        let deadline = tokio::time::Instant::now() + TURN_TIMEOUT;
        let run_id = match start_run(node, &turn.request, tx, cancel).await {
            Ok(run_id) => run_id,
            Err(err) => return (None, StreamEnd::Seen(Err(err))),
        };
        let _streamed = self.streaming_turn(&turn.agent.to_hex(), run_id.clone());
        let (socket, head) = match self.send_prompt(node, &run_id, turn).await {
            Ok(sent) => sent,
            Err(err) => return (Some(run_id), StreamEnd::Seen(Err(err))),
        };
        let _ = tx.send(RemoteEvent::Status(WORKING.into())).await;
        let watch = RunWatch {
            hub: self,
            node,
            agent: turn.agent,
            run_id: &run_id,
            head,
        };
        let end = watch
            .follow(Some(socket), Shown::Live, tx, cancel, deadline)
            .await;
        (Some(run_id), end)
    }

    /// Attach to run `run_id` and send it `turn`'s prompt, marking the
    /// turn before it goes out: a hub that dies as it sends leaves the
    /// turn marked, for the next to follow, never a turn running unmarked.
    /// A prompt that could not be sent started no turn: its mark goes.
    /// Returns the socket and the head of the record the turn writes
    /// after.
    async fn send_prompt(
        &self,
        node: &NodeClient,
        run_id: &str,
        turn: &TurnPrompt,
    ) -> CloudResult<(NodeSocket, u64)> {
        let mut socket = connect_run(node, run_id).await?;
        // What the turn writes comes after the node's head now.
        let head = match node.head(turn.agent).await {
            Ok(head) => head,
            Err(_) => self.store.get_head_seq(turn.agent).unwrap_or(0),
        };
        let message = InboundMessage::UserMessage(UserMessage {
            content: turn.prompt.clone(),
            tool_hints: None,
            attachments: None,
        });
        let text = serde_json::to_string(&message).unwrap_or_default();
        let mark = TurnMark::new(run_id, head, &turn.prompt, &turn.request);
        self.send_marked(turn.agent, mark, socket.send(Message::Text(text.into())))
            .await
            .map_err(|err| CloudError::Transport {
                endpoint: node.base().to_string(),
                message: err.to_string(),
            })?;
        Ok((socket, head))
    }

    /// Mark `agent`'s turn `mark`, then `send` its prompt. When the send
    /// fails the turn never started, and its mark goes again.
    pub(super) async fn send_marked<E>(
        &self,
        agent: AgentId,
        mark: TurnMark,
        send: impl std::future::Future<Output = Result<(), E>>,
    ) -> Result<(), E> {
        let run_id = mark.run_id.clone();
        self.mark_turn(agent, mark);
        let sent = send.await;
        if sent.is_err() {
            self.end_turn_mark(agent, &run_id);
        }
        sent
    }

    /// After `agent`'s turn on `run_id` ended as `end` says: copy what it
    /// wrote, its session and its runs while `guard` holds the machine
    /// awake, let the turn's mark go and say how it went. A turn the hub
    /// let go of runs on, its mark kept for the next hub.
    pub(super) async fn finish_turn(
        &self,
        node: &NodeClient,
        agent: AgentId,
        run_id: Option<&str>,
        end: StreamEnd,
        guard: WorkGuard,
        tx: &mpsc::Sender<RemoteEvent>,
    ) {
        let StreamEnd::Seen(result) = end else {
            return;
        };
        self.finish_on_node(node, agent).await;
        if let Some(run_id) = run_id {
            self.end_turn_mark(agent, run_id);
        }
        drop(guard);
        if let Err(err) = result {
            let _ = tx.send(RemoteEvent::Failed(err.to_string())).await;
        }
    }

    /// Copy what `agent`'s turn on `node` left: its record, its session
    /// checkpoint and its trace runs. Hold the machine awake around it.
    pub(super) async fn finish_on_node(&self, node: &NodeClient, agent: AgentId) {
        let hex = agent.to_hex();
        if let Err(err) = self.mirror_from(node, agent).await {
            tracing::warn!(agent = %hex, error = %err, "could not copy the turn's record");
        }
        if let Err(err) = self.copy_session(node, agent).await {
            tracing::debug!(agent = %hex, error = %err, "could not copy the session checkpoint");
        }
        if let Err(err) = self.copy_traces(node, agent).await {
            tracing::debug!(agent = %hex, error = %err, "could not copy the trace runs");
        }
    }
}

/// What a turn's stream says once its machine took the turn.
pub const WORKING: &str = "Working in the cloud";

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

/// Start the chat run `request` asks for on `node` once its agent has no
/// other turn under way there, showing the wait on `tx`. Returns its id.
async fn start_run(
    node: &NodeClient,
    request: &RuntimeRequest,
    tx: &mpsc::Sender<RemoteEvent>,
    cancel: &CancellationToken,
) -> CloudResult<String> {
    let started = super::busy::start_when_free(node, request, tx, cancel).await?;
    started
        .get("run_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| CloudError::InvalidResponse {
            op: "run",
            message: "no run id".into(),
        })
}

/// Open the event stream of chat run `run_id` on `node`: its history
/// first, then what it sends from now on.
pub(super) async fn connect_run(node: &NodeClient, run_id: &str) -> CloudResult<NodeSocket> {
    let url = ws_url(node.base(), &format!("/stream/{run_id}"));
    let connect = tokio_tungstenite::connect_async(ws_request(&url, node.token())?);
    match tokio::time::timeout(NODE_TIMEOUT, connect).await {
        Ok(Ok((socket, _))) => Ok(socket),
        Ok(Err(err)) => Err(CloudError::Transport {
            endpoint: url,
            message: err.to_string(),
        }),
        Err(_) => Err(CloudError::Timeout {
            op: "attach to the turn",
            after: NODE_TIMEOUT,
        }),
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
        OutboundMessage::ToolOutput(output) => RemoteEvent::ToolOutput {
            id: output.id,
            chunk: output.chunk,
        },
        OutboundMessage::ToolResult(result) => RemoteEvent::ToolResult {
            id: result.tool_use_id.unwrap_or_default(),
            result: result.result,
            is_error: result.is_error,
        },
        OutboundMessage::AssistantMessageEnd(end) => RemoteEvent::Done(Box::new(end.usage)),
        OutboundMessage::ChatTitle(title) => RemoteEvent::Title(title.title),
        OutboundMessage::IterationComplete(_) => RemoteEvent::ModelCallEnd,
        OutboundMessage::Error(err) if !err.recoverable => RemoteEvent::Failed(err.message),
        _ => return None,
    })
}
