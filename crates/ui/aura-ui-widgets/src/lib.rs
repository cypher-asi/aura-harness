//! # aura-ui-widgets
//!
//! Layer: ui
//!
//! Terminal widgets, themes, graphics and animation that the terminal UI
//! draws with. Nothing here holds application state: each module turns
//! values into ratatui lines, spans and buffer cells (or, for `graphics`
//! and `clipboard`, terminal escape sequences), so `aura-ui-terminal`
//! builds on it and it compiles alongside the rest of the workspace.

#![forbid(unsafe_code)]

pub mod animation;
pub mod ansi;
pub mod clipboard;
pub mod components;
pub mod graphics;
pub mod themes;
