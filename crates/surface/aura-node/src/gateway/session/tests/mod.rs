//! Session-state unit tests.
//!
//! These were extracted from `session/mod.rs` in Wave 6 / T3 so the
//! module-root file can stay tiny (declarations + `WsContext` + the
//! `Session` re-export).
//!
//! Phase A: the `SessionInit` first-frame contract was replaced with
//! `POST /v1/run` + `WS /stream/:run_id`. These tests now drive
//! [`Session::apply_chat_runtime_request`] directly with chat-shaped
//! [`RuntimeRequest`] payloads built by [`chat_request`].

mod chat_request;
mod project_path;
mod storage;
mod stream_timeout;
mod support;
