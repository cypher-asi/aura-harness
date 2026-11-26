//! # aura-engine-automaton
//!
//! Layer: engine
//!
//! Headless automaton host: built-in flows (`DevLoopAutomaton`,
//! `SpecGenAutomaton`, `TaskRunAutomaton`), the `AutomatonRuntime`
//! scheduler, the `TickContext` injection point, and the
//! `AutomatonEvent` event surface. `aura-node` depends on it
//! directly.
#![forbid(unsafe_code)]

mod context;
mod error;
mod events;
mod handle;
mod metadata;
mod runtime;
mod state;
mod types;

pub mod builtins;

pub use context::TickContext;
// `AutomatonError` is internal to the crate (no external crate names it
// directly — `aura-node` only consumes it via Display in `.map_err`).
// Kept as `pub(crate) use` so the in-crate path `crate::AutomatonError`
// keeps working without bumping the public API surface.
pub(crate) use error::AutomatonError;
pub use events::AutomatonEvent;
pub use handle::AutomatonHandle;
pub use runtime::{Automaton, AutomatonRuntime, TickOutcome};
// `Schedule` (in `crate::metadata`), `AutomatonStatus`, `ChatAutomaton`
// are internal to the crate; nothing outside `aura-engine-automaton` references
// them by name. Their underlying `pub` definitions stay reachable
// through `AutomatonInfo` fields / trait return types without being
// part of the crate's named public surface.
pub use state::AutomatonState;
pub use types::{AutomatonId, AutomatonInfo};

pub use builtins::{DevLoopAutomaton, SpecGenAutomaton, TaskRunAutomaton};
