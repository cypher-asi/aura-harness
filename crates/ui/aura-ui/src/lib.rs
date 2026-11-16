//! # aura-ui
//!
//! Layer: ui
//!
//! What every AURA frontend shares, independent of how it draws: the
//! [`UiEvent`] / [`UiCommand`] protocol between a UI and the agent event
//! loop, input history, tool-call formatting, the sandbox file tree and
//! the `/mode` parser. The ratatui terminal (`aura-ui-terminal`) and the
//! native desktop app (`aura-desktop`) both build on it.

#![forbid(unsafe_code)]
#![warn(clippy::all)]

pub mod events;
pub mod format;
pub mod history;
pub mod mode_command;
pub mod sandbox;

pub use events::{UiCommand, UiEvent};
pub use history::InputHistory;
pub use mode_command::{ModeCommandError, SlashModeCommand};
