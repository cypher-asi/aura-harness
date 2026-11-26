//! Periodic `tool_running` progress heartbeat while a tool batch runs.

use std::time::{Duration, Instant};

use tokio::sync::mpsc::Sender;
use tokio::task::JoinHandle;

use crate::events::AgentLoopEvent;
use crate::types::ToolCallInfo;

/// Resolved heartbeat cadence. Reads
/// `aura_config::agent().tools.heartbeat_interval` (which is
/// `AURA_TURN_TOOL_HEARTBEAT_INTERVAL_SECS`, clamped at
/// `aura_config` boundaries). Same value the aura-os side reads, so
/// the harness emits a heartbeat well inside the server's
/// sliding-idle window (`AURA_TURN_MAX_TIMEOUT_SECS`, default 180s).
///
/// Phase 7 dropped the only production caller (the buffered
/// transport's `prepare_live_batch`); the heartbeat infrastructure
/// is kept for the buffered-style helpers' unit-test coverage and
/// for any future transport (e.g. cassette replay) that needs
/// inline executor calls.
pub(crate) fn tool_heartbeat_interval() -> Duration {
    aura_config::agent().tools.heartbeat_interval
}

/// Spawn a background task that emits an
/// [`AgentLoopEvent::Progress`] heartbeat every
/// [`tool_heartbeat_interval`] while a batch of tool calls is in
/// flight. Returns a [`HeartbeatGuard`] whose `Drop` aborts the task
/// — callers get cancel-on-completion semantics for free without
/// having to wire `tokio::select!` around every executor call.
///
/// The first heartbeat fires after the first interval tick (i.e. the
/// "cool" 0..interval window stays silent), so a tool that completes
/// inside the interval never emits one. After that the cadence is
/// strictly periodic; each tick reports `tool_running`,
/// `tool_name=<first-tool-of-batch>`, `elapsed_ms` since batch start.
///
/// `event_tx` is `None` for the headless code path (no event channel,
/// e.g. unit tests in non-streaming mode); the spawn is skipped and
/// the returned guard is a no-op so the call site stays branch-free.
pub(in crate::agent_loop) fn spawn_tool_heartbeat(
    event_tx: Option<&Sender<AgentLoopEvent>>,
    to_execute: &[ToolCallInfo],
    interval: Duration,
) -> HeartbeatGuard {
    let Some(tx) = event_tx.cloned() else {
        return HeartbeatGuard { handle: None };
    };
    if to_execute.is_empty() {
        return HeartbeatGuard { handle: None };
    }
    // Most batches contain a single tool call; when the model emits
    // multiple in one turn we report the first one's name on the
    // heartbeat. The server-side watchdog only cares about *some*
    // forward-progress event arriving, and the chat client renders
    // the stage label, so a single representative name is enough to
    // keep the UI honest without generating one heartbeat per tool.
    let tool_name = to_execute[0].name.clone();
    let started_at = Instant::now();
    let handle = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        // The first tick fires immediately; consume it so the heartbeat
        // only emits after the configured wall-clock window has
        // elapsed (matching the documented "long tool calls > 10s"
        // contract).
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let elapsed_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
            let event = AgentLoopEvent::Progress {
                stage: "tool_running".to_string(),
                tool_name: Some(tool_name.clone()),
                elapsed_ms: Some(elapsed_ms),
                message: None,
            };
            // `send` (with backpressure) over `try_send` so a
            // momentarily-full broadcast doesn't drop a heartbeat —
            // dropping one defeats the watchdog-friendliness contract.
            if tx.send(event).await.is_err() {
                // Receiver gone (turn already finalized): stop ticking.
                break;
            }
        }
    });
    HeartbeatGuard {
        handle: Some(handle),
    }
}

/// RAII guard for [`spawn_tool_heartbeat`]. Aborts the heartbeat task
/// on drop so a panicking executor or an early `?`-return at the
/// caller doesn't leak the periodic emission past the tool's
/// lifetime.
pub(in crate::agent_loop) struct HeartbeatGuard {
    handle: Option<JoinHandle<()>>,
}

impl Drop for HeartbeatGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}
