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

pub use mapper::{map_agent_loop_event, TurnEventSink};
pub use types::{AgentLoopEvent, TurnEvent};
pub use wire::DebugEvent;
