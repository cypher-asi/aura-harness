//! # aura-app
//!
//! Layer: surface
//!
//! The agent session host shared by every interactive surface. It opens
//! the agent registry, record store, model provider and kernel factory
//! for a workspace and runs the event loop between them and a UI:
//!
//! - [`session::start`] serves the ratatui terminal (`aura-cli`) and the
//!   native window (`aura-desktop`) over a pair of UI channels.
//! - [`headless::run`] runs one prompt for `aura -p` and prints the
//!   result.
//!
//! Argument parsing stays in the binaries: they turn their flags into
//! [`session::SessionOptions`] or [`headless::HeadlessOptions`].

#![forbid(unsafe_code)]
#![warn(clippy::all)]
#![allow(
    clippy::manual_let_else,
    clippy::map_unwrap_or,
    clippy::cast_possible_wrap,
    clippy::ref_option,
    clippy::needless_pass_by_value,
    clippy::unnecessary_wraps
)]

pub mod agent_definitions;
pub mod agent_registry;
pub mod api_server;
pub mod bench_pack;
pub mod cloud;
pub mod event_loop;
pub mod file_checkpoints;
mod harness;
pub mod headless;
pub mod isolation;
mod long_run;
pub mod model_selection;
pub mod priority;
pub mod project_instructions;
pub mod record_loader;
pub mod recorded_runs;
pub mod repo_source;
pub mod session;
pub mod session_checkpoint;
pub mod session_helpers;
mod turn_calls;

pub use long_run::parse_duration;
