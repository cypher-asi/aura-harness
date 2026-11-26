//! Helper functions for WebSocket session management: init, executor
//! construction, event forwarding, and turn finalization.

mod finalize;
mod init;
mod kernel;
mod sink;
#[cfg(test)]
mod tests;

pub(crate) use init::{prepare_chat_session, ChatRequestError};

#[cfg(test)]
pub(super) use finalize::apply_turn_result;
pub(super) use finalize::{finalize_turn, trace_recorder};
pub(super) use init::{emit_session_ready, session_user_defaults};
pub(super) use kernel::{build_kernel_with_config, register_session_agent};
pub(super) use sink::{forward_and_record, forward_events_to_ws};
