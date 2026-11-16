//! Unified streaming events emitted during agent execution.
//!
//! [`TurnEvent`] is the event type for `AgentLoop`. Consumers subscribe
//! by passing an `mpsc::Sender<TurnEvent>` to the orchestrator.
//!
//! Debug observability (`debug.*`) events are carried through the same
//! channel via the [`TurnEvent::Debug`] variant, so a single consumer
//! sees both the live UI stream and the structured metrics stream in
//! arrival order. The [`DebugEvent`] type itself is JSON-tagged
//! (`{"type": "debug.llm_call", ...}`) to match the on-disk schema the
//! `aura-os` run-log consumer expects — see
//! `apps/aura-os-server/src/loop_log.rs::update_counters` in the
//! sibling repo.
//!
//! The [`mapper`] submodule provides a shared `TurnEventSink` trait +
//! [`map_agent_loop_event`] dispatcher used by both the TUI's
//! `UiCommandSink` and the headless WebSocket session's
//! `OutboundMessageSink` so adding a new `TurnEvent` variant is a
//! compile error until every consumer handles it.
//!
//! # Module layout
//!
//! - [`types`] — the in-process [`TurnEvent`] enum and its
//!   `AgentLoopEvent` alias.
//! - [`wire`] — the JSON-tagged [`DebugEvent`] frames that flow over
//!   the same channel and end up in the `aura-os` run bundle.
//! - [`mapper`] — the [`TurnEventSink`] trait and [`map_agent_loop_event`]
//!   dispatcher that fans events out to UI and WebSocket consumers.
//! - [`tests`] — wire/round-trip tests for [`DebugEvent`].

pub mod mapper;
mod types;
mod wire;

#[cfg(test)]
mod tests;

/// `Progress` stage sent after a `todo_write` call; `message` is the
/// full list as JSON (`Vec<aura_tools::todo::TodoItem>`).
pub const PROGRESS_TODOS_UPDATED: &str = "todos_updated";
/// `Progress` stage sent after the transcript was summarized to free
/// context; `message` is a short human-readable description.
pub const PROGRESS_CONTEXT_SUMMARIZED: &str = "context_summarized";

/// `Error` code sent when the model's route refused the sign-in or plan
/// behind the request (401 or 403); a 402 is `insufficient_credits`.
pub const ERROR_UNAUTHORIZED: &str = "unauthorized";
/// `Error` code sent when the route has no credits left (402).
pub const ERROR_INSUFFICIENT_CREDITS: &str = "insufficient_credits";

pub use mapper::{map_agent_loop_event, TurnEventSink};
pub use types::{AgentLoopEvent, TurnEvent};
pub use wire::DebugEvent;
