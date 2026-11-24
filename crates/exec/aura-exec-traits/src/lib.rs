//! # aura-exec-traits
//!
//! Layer: exec
//!
//! The exec layer's seams: the executor and spawn-hook traits, the hooks
//! tools call out through (agent control, subagent dispatch, file changes,
//! background processes, automaton control), and the [`domain::DomainApi`]
//! the domain tools call.
//!
//! The crate depends only on `aura-core-types` (plus non-aura deps), so the
//! tool catalog, the kernel, and every hook implementor can depend on it
//! without depending on each other.

#![forbid(unsafe_code)]
#![warn(clippy::all)]

pub mod domain;
mod executor;
mod hooks;
mod router;
mod scratch;
mod spawn_hook;

pub use executor::{
    decode_tool_effect, DecodedToolResult, ExecuteContext, ExecuteLimits, Executor, ExecutorError,
};
pub use hooks::{
    AgentControlHook, AgentReadHook, AutomatonController, BackgroundJob, BackgroundProcess,
    BackgroundProcessHook, FileChangeHook, SubagentDispatchHook,
};
pub use router::ExecutorRouter;
pub use scratch::RunScratch;
pub use spawn_hook::{ChildAgentSpec, NoopSpawnHook, SpawnError, SpawnHook, SpawnOutcome};
