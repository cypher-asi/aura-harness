//! Forward-event translation tests for the dev-loop.
//!
//! The simplification removed `TaskAggregate`, `validate_execution`,
//! and `commit_and_push` along with the tests that covered them. The
//! `forward_agent_event` translation layer is still load-bearing for
//! the WS event stream consumed by chat, dev-loop, and task_run, so
//! those tests stay.
//!
//! `AgentIdentityEnvelope` wire-roundtrip tests live here too: they
//! lock the JSON shape aura-os populates → harness parses → rendered
//! system prompt tags so a future schema drift on either side
//! triggers a compile / test failure rather than a silent
//! cross-repo break.

use crate::builtins::common::{
    forward_agent_event, spawn_agent_event_forwarder, AgentIdentityEnvelope, ForwardOutcome,
};

use crate::events::AutomatonEvent;

mod envelope;
mod failures;
mod forwarder;
mod forwarding;
