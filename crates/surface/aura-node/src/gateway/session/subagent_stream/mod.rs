//! Runtime-layer subagent observability: make each `task` child run
//! observable as its own live WS-attachable thread.
//!
//! Layer boundary: this module is the ONLY place that turns subagent
//! activity into `aura_core_protocol::OutboundMessage` frames and registers
//! a child run id in the [`super::chat_run::ChatRunRegistry`]. The
//! engine's agent host only forwards an opaque
//! [`AgentLoopEvent`](aura_agent::AgentLoopEvent) sink; it never
//! constructs wire messages.
//!
//! Flow per `task` dispatch:
//! 1. Mint a `child_run_id` (UUID).
//! 2. Register an event-only [`super::chat_run::ChatRunHandle`] under
//!    that id so `WS /stream/:child_run_id` can attach (replay + live).
//! 3. Spawn a forwarder mapping the child loop's `AgentLoopEvent`s onto
//!    the child run's `OutboundMessage` stream
//!    ([`super::helpers::forward_events_to_ws`]).
//! 4. Emit `SubagentSpawned` on the PARENT stream BEFORE the (blocking
//!    Wait) dispatch.
//! 5. Delegate to the inner [`EventAwareSubagentDispatch`], threading
//!    the child event sink + the parent turn cancellation token.
//! 6. On completion, emit `SubagentStatus` on the parent stream, push a
//!    terminal status into the child stream, and schedule the child run
//!    for reaping after the idle-retention grace window.

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use aura_agent::AgentLoopEvent;
use aura_core_types::{SubagentDispatchRequest, SubagentExit, SubagentResult};
use aura_engine::AgentHooks;
use aura_exec_tools::SubagentDispatchHook;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::chat_run::{
    register_run, spawn_event_forwarder, ChatEventChannel, ChatRunRegistry, RunLinkage,
    RunRegistration, CHAT_RUN_IDLE_RETENTION,
};
use crate::protocol::{InboundMessage, OutboundMessage, SubagentSpawned, SubagentStatus};

/// Capacity of the per-child [`AgentLoopEvent`] channel. Mirrors the
/// parent turn channel in [`super::chat::dispatch_turn_to_agent`].
const CHILD_EVENT_CHANNEL_CAPACITY: usize = 1024;

/// Capacity of the event-only run's (unused) inbound command channel.
const CHILD_COMMAND_CHANNEL_CAPACITY: usize = 8;

/// Observability-aware dispatch surface implemented by
/// [`AgentHooks`]. Carved out as a trait so the runtime hook can wrap a
/// test double without standing up an agent host.
#[async_trait]
pub(crate) trait EventAwareSubagentDispatch: Send + Sync {
    /// Dispatch the child run, threading an optional streaming sink and
    /// the parent turn cancellation token.
    async fn dispatch_with_events(
        &self,
        request: SubagentDispatchRequest,
        event_tx: Option<mpsc::Sender<AgentLoopEvent>>,
        cancellation: Option<CancellationToken>,
    ) -> Result<SubagentResult, String>;
}

#[async_trait]
impl EventAwareSubagentDispatch for AgentHooks {
    async fn dispatch_with_events(
        &self,
        request: SubagentDispatchRequest,
        event_tx: Option<mpsc::Sender<AgentLoopEvent>>,
        cancellation: Option<CancellationToken>,
    ) -> Result<SubagentResult, String> {
        AgentHooks::dispatch_with_events(self, request, event_tx, cancellation).await
    }
}

/// Wraps an inner subagent dispatcher and makes each child run
/// observable on the wire. See the module docs for the per-dispatch
/// flow.
pub(crate) struct RuntimeSubagentObservabilityHook {
    inner: Arc<dyn EventAwareSubagentDispatch>,
    parent_outbound: mpsc::Sender<OutboundMessage>,
    chat_runs: ChatRunRegistry,
    parent_cancellation: Option<CancellationToken>,
    /// Run id of the parent (top-level) chat run, stamped onto each
    /// child's linkage as `parent_run_id`.
    parent_run_id: Option<String>,
    /// How long a completed child run lingers in the shared registry
    /// before being reaped (see [`schedule_child_run_cleanup`]).
    /// Defaults to [`CHAT_RUN_IDLE_RETENTION`]; overridable in tests.
    cleanup_retention: Duration,
    /// Council combine mechanism (`synthesize` / `contrast` /
    /// `side_by_side`) stamped onto every council-member `SubagentSpawned`
    /// this hook emits (gated by `council_index`, like `model`). `None`
    /// for the ordinary `task`-tool hook, which emits no council members.
    council_mechanism: Option<String>,
}

impl RuntimeSubagentObservabilityHook {
    /// Construct a hook that emits onto `parent_outbound`, registers
    /// child runs in `chat_runs`, forks `parent_cancellation` into each
    /// `Wait` child, and stamps `parent_run_id` onto child linkage.
    pub(crate) fn new(
        inner: Arc<dyn EventAwareSubagentDispatch>,
        parent_outbound: mpsc::Sender<OutboundMessage>,
        chat_runs: ChatRunRegistry,
        parent_cancellation: Option<CancellationToken>,
        parent_run_id: Option<String>,
    ) -> Self {
        Self {
            inner,
            parent_outbound,
            chat_runs,
            parent_cancellation,
            parent_run_id,
            cleanup_retention: CHAT_RUN_IDLE_RETENTION,
            council_mechanism: None,
        }
    }

    /// Stamp the AURA Council combine mechanism onto every council-member
    /// `SubagentSpawned` this hook emits. Set by the council coordinator;
    /// left unset by the ordinary `task`-tool path.
    pub(crate) fn with_council_mechanism(mut self, mechanism: Option<String>) -> Self {
        self.council_mechanism = mechanism;
        self
    }

    /// Override the post-completion registry retention. Test-only: the
    /// production default ([`CHAT_RUN_IDLE_RETENTION`]) is far too long
    /// to exercise reaping in a unit test.
    #[cfg(test)]
    fn with_cleanup_retention(mut self, retention: Duration) -> Self {
        self.cleanup_retention = retention;
        self
    }

    /// Per-child cancellation token forked off the parent turn token (a
    /// standalone token when no parent token is present). Stored as the
    /// child run's `shutdown` so `POST /v1/run/:id/stop` cancels only
    /// this child, while a parent-turn cancellation still propagates
    /// down through the forked token.
    fn child_cancellation(&self) -> CancellationToken {
        match &self.parent_cancellation {
            Some(parent) => parent.child_token(),
            None => CancellationToken::new(),
        }
    }

    /// Register a child run under `child_run_id` through the shared
    /// run-handle path ([`register_run`]) with parent `linkage`, and
    /// return the [`AgentLoopEvent`] sink the child loop streams into
    /// plus the run's event channel (so the caller can push a terminal
    /// status and mark it done).
    fn register_child_run(
        &self,
        child_run_id: &str,
        shutdown: CancellationToken,
        linkage: RunLinkage,
    ) -> (
        mpsc::Sender<AgentLoopEvent>,
        Arc<ChatEventChannel>,
        tokio::task::JoinHandle<()>,
    ) {
        let channel = ChatEventChannel::new();
        let child_outbound = spawn_event_forwarder(channel.clone());

        // Event-only run: no driver consumes inbound commands, so the
        // receiver is dropped immediately. A WS attach can still replay
        // history + stream live; inbound frames simply close that
        // attach's reader. The handle is registered through the same
        // `register_run` path a top-level `POST /v1/run` uses, so the
        // child appears in the shared registry like a real run (with
        // parent linkage) and is lookup/attach/stop-able the same way.
        let (commands, _commands_rx) =
            mpsc::channel::<InboundMessage>(CHILD_COMMAND_CHANNEL_CAPACITY);
        register_run(
            &self.chat_runs,
            child_run_id.to_string(),
            RunRegistration {
                commands,
                events: channel.clone(),
                attach_count: Arc::new(AtomicUsize::new(0)),
                shutdown,
                linkage: Some(linkage),
                agent: None,
                activity: Arc::default(),
            },
        );

        let (event_tx, event_rx) = mpsc::channel::<AgentLoopEvent>(CHILD_EVENT_CHANNEL_CAPACITY);
        // The forwarder ends when the child loop drops its event sender,
        // i.e. when the child run finishes — the detached path awaits this
        // handle to emit a terminal status without waiting on the child's
        // result (which goes to the parent as a message).
        let forwarder = tokio::spawn(super::helpers::forward_events_to_ws(
            event_rx,
            child_outbound,
        ));

        (event_tx, channel, forwarder)
    }
}

impl RuntimeSubagentObservabilityHook {
    /// Build the `SubagentSpawned` announcement for `request`.
    fn spawned_event(
        &self,
        request: &SubagentDispatchRequest,
        child_run_id: &str,
    ) -> SubagentSpawned {
        // AURA Council members share ONE `council_parent_tool_use_id` so the
        // UI folds them into a single council panel; that shared id is what
        // we advertise as `parent_tool_use_id` (and stamp on child linkage),
        // NOT each member's own `tool_call_id` (which stays unique/none so the
        // `(parent, tool_call_id)` dedupe never collapses two members). An
        // ordinary `task` spawn leaves `council_parent_tool_use_id` unset and
        // keeps deriving the id from `tool_call_id` exactly as before.
        let parent_tool_use_id = request
            .council_parent_tool_use_id
            .clone()
            .or_else(|| request.tool_call_id.clone());
        // Council labelling: a council-member dispatch carries
        // `council_index = Some(i)`. Both `model` and `council_index` on
        // the emitted `SubagentSpawned` are gated behind that being
        // `Some`, so an ordinary `task` spawn (which may legitimately set
        // `model_override` for a non-default model) keeps emitting
        // `model: None, council_index: None` exactly as before — the
        // least-surprising behavior, avoiding mislabelling task spawns as
        // council members.
        let council_index = request.council_index;
        let council_model = council_index.and(request.model_override.clone());
        // Gate the mechanism behind `council_index` too so an ordinary
        // `task` spawn never carries it (mirrors `council_model`).
        let council_mechanism = council_index.and(self.council_mechanism.clone());
        SubagentSpawned {
            child_run_id: child_run_id.to_string(),
            parent_tool_use_id,
            subagent_type: request.subagent_type.clone(),
            prompt: request.prompt.clone(),
            model: council_model,
            council_index,
            council_mechanism,
        }
    }

    /// Parent linkage stamped onto the shared registry entry. `depth`
    /// is the count of ancestors carried on the request's
    /// `parent_chain` (which already includes the spawning parent).
    fn run_linkage(
        &self,
        request: &SubagentDispatchRequest,
        parent_tool_use_id: Option<String>,
        child_run_id: &str,
    ) -> RunLinkage {
        RunLinkage {
            parent_run_id: self.parent_run_id.clone(),
            parent_tool_use_id,
            child_run_id: child_run_id.to_string(),
            depth: request.parent_chain.len(),
            parent_chain: request
                .parent_chain
                .iter()
                .map(ToString::to_string)
                .collect(),
        }
    }

    /// Detached dispatch returns an immediate ack while the child is
    /// still running in the background. Reflect `running` now and emit
    /// the terminal status when the child's event stream closes — NOT
    /// a premature `completed` derived from the ack.
    fn finish_detached(
        &self,
        forwarder: tokio::task::JoinHandle<()>,
        child_channel: Arc<ChatEventChannel>,
        child_run_id: String,
    ) {
        let running = SubagentStatus {
            child_run_id: child_run_id.clone(),
            state: "running".to_string(),
            reason: None,
        };
        let _ = self
            .parent_outbound
            .try_send(OutboundMessage::SubagentStatus(running.clone()));
        child_channel.push(OutboundMessage::SubagentStatus(running));
        spawn_detached_completion_watch(
            forwarder,
            self.parent_outbound.clone(),
            child_channel,
            self.chat_runs.clone(),
            child_run_id,
            self.cleanup_retention,
        );
    }

    fn finish_waited(
        &self,
        forwarder: tokio::task::JoinHandle<()>,
        child_channel: &ChatEventChannel,
        child_run_id: String,
        result: &Result<SubagentResult, String>,
    ) {
        let status = status_payload(&child_run_id, result);
        let _ = self
            .parent_outbound
            .try_send(OutboundMessage::SubagentStatus(status.clone()));

        // Terminate the child stream cleanly: push the terminal status
        // into its replay history, then mark it done so a late attach
        // replays the full thread without blocking on a live receiver.
        child_channel.push(OutboundMessage::SubagentStatus(status));
        child_channel.mark_done();
        // Wait path: the forwarder is already draining to completion as
        // the child loop dropped its sender; detaching the handle is
        // equivalent to the prior fire-and-forget spawn.
        drop(forwarder);
        schedule_child_run_cleanup(self.chat_runs.clone(), child_run_id, self.cleanup_retention);
    }
}

#[async_trait]
impl SubagentDispatchHook for RuntimeSubagentObservabilityHook {
    async fn dispatch(&self, request: SubagentDispatchRequest) -> Result<SubagentResult, String> {
        let child_run_id = Uuid::new_v4().to_string();
        let spawned = self.spawned_event(&request, &child_run_id);
        let is_detached = matches!(
            request.spawn_mode,
            Some(aura_core_types::SpawnMode::Detached)
        );
        let linkage = self.run_linkage(&request, spawned.parent_tool_use_id.clone(), &child_run_id);

        // One token serves as both the child's `shutdown` (so a `stop`
        // cancels just this child) and the dispatch cancellation (so a
        // parent-turn cancel still propagates). Handing the host a
        // standalone uncancelled token when there is no parent token is
        // equivalent to passing `None`.
        let child_cancel = self.child_cancellation();
        let (event_tx, child_channel, forwarder) =
            self.register_child_run(&child_run_id, child_cancel.clone(), linkage);

        // Emit `SubagentSpawned` on the parent stream BEFORE the (Wait)
        // dispatch blocks so the client can render a clickable thread
        // card and lazily attach to `child_run_id`.
        let _ = self
            .parent_outbound
            .try_send(OutboundMessage::SubagentSpawned(spawned));

        let result = self
            .inner
            .dispatch_with_events(request, Some(event_tx), Some(child_cancel))
            .await;

        if is_detached && result.is_ok() {
            self.finish_detached(forwarder, child_channel, child_run_id);
            return result;
        }
        self.finish_waited(forwarder, &child_channel, child_run_id, &result);
        result
    }
}

/// Watch a detached child's event forwarder to completion, then publish
/// the terminal status on both the parent and child streams and reap the
/// run. The typed exit is not available here (the result goes to the
/// parent as a message), so a generic `completed` terminal is emitted;
/// any failure detail still streams through the child's live events.
fn spawn_detached_completion_watch(
    forwarder: tokio::task::JoinHandle<()>,
    parent_outbound: mpsc::Sender<OutboundMessage>,
    child_channel: Arc<ChatEventChannel>,
    chat_runs: ChatRunRegistry,
    child_run_id: String,
    cleanup_retention: Duration,
) {
    tokio::spawn(async move {
        let _ = forwarder.await;
        let status = SubagentStatus {
            child_run_id: child_run_id.clone(),
            state: "completed".to_string(),
            reason: None,
        };
        let _ = parent_outbound.try_send(OutboundMessage::SubagentStatus(status.clone()));
        child_channel.push(OutboundMessage::SubagentStatus(status));
        child_channel.mark_done();
        schedule_child_run_cleanup(chat_runs, child_run_id, cleanup_retention);
    });
}

/// Map a dispatch outcome onto the wire `SubagentStatus` payload. The
/// `state` string matches the documented vocabulary on
/// [`SubagentStatus`]: `running | completed | failed | cancelled |
/// timeout | rejected`.
fn status_payload(child_run_id: &str, result: &Result<SubagentResult, String>) -> SubagentStatus {
    let (state, reason) = match result {
        Ok(res) => match &res.exit {
            SubagentExit::Completed => ("completed", None),
            SubagentExit::Cancelled => ("cancelled", None),
            SubagentExit::Timeout => ("timeout", None),
            SubagentExit::Failed { reason } => ("failed", Some(reason.clone())),
            SubagentExit::Rejected { reason } => ("rejected", Some(reason.clone())),
        },
        Err(err) => ("failed", Some(err.clone())),
    };
    SubagentStatus {
        child_run_id: child_run_id.to_string(),
        state: state.to_string(),
        reason,
    }
}

/// Reap the child run from the registry after the retention window so a
/// client that attached mid-run can still replay the completed thread,
/// but the entry does not leak forever. A child run lingers for the
/// grace window rather than being removed synchronously on completion
/// (unlike a top-level run, whose own driver self-removes), because the
/// terminal thread must stay replayable for a late `WS /stream/:id`
/// attach; an explicit `POST /v1/run/:id/stop` still removes it eagerly.
fn schedule_child_run_cleanup(
    chat_runs: ChatRunRegistry,
    child_run_id: String,
    retention: Duration,
) {
    tokio::spawn(async move {
        tokio::time::sleep(retention).await;
        chat_runs.remove(&child_run_id);
    });
}

#[cfg(test)]
mod tests;
