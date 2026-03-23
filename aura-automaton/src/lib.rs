#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]
#![allow(dead_code)]

mod context;
mod error;
mod events;
mod handle;
mod runtime;
mod schedule;
mod state;
mod types;

pub mod builtins;

pub use context::TickContext;
pub use error::AutomatonError;
pub use events::AutomatonEvent;
pub use handle::AutomatonHandle;
pub use runtime::AutomatonRuntime;
pub use schedule::Schedule;
pub use state::AutomatonState;
pub use types::{AutomatonId, AutomatonInfo, AutomatonStatus};

pub use builtins::{ChatAutomaton, DevLoopAutomaton, SpecGenAutomaton, TaskRunAutomaton};
