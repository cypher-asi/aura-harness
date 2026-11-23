//! # aura-ui-terminal
//!
//! Layer: ui
//!
//! Cyber-retro terminal UI library for AURA CLI.
//!
//! This crate provides a standalone terminal UI that can be embedded in any
//! Rust application wanting the AURA terminal experience.
//!
//! ## Features
//!
//! - **Cyber Aesthetic**: Neon colors, ASCII art, box-drawing UI
//! - **Rich Components**: Messages, tool cards, diffs, modals
//! - **Themes**: Cyber (default), Matrix, Synthwave, Minimal
//! - **Animations**: Spinners, progress bars, streaming text
//! - **Responsive Layout**: Adapts from 40-char to 200-char terminals
//! - **Input System**: History, autocomplete, slash commands
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use aura_ui_terminal::{Terminal, App, Theme, TerminalError};
//!
//! fn main() -> Result<(), TerminalError> {
//!     let theme = Theme::cyber();
//!     let mut terminal = Terminal::new(theme)?;
//!     let mut app = App::new();
//!
//!     terminal.run(&mut app)?;
//!     Ok(())
//! }
//! ```

#![forbid(unsafe_code)]
#![warn(clippy::all)]

pub mod alerts;
pub mod animation;
pub mod ansi;
pub mod components;
pub use aura_ui::events;
pub mod input;
pub mod layout;
pub use aura_ui::mode_command;
pub mod themes;

mod app;
mod clipboard;
mod graphics;
mod renderer;
mod terminal;

// Re-exports for convenience
pub use alerts::AlertSettings;
/// The GRAPH tab's cards and wires, for a front end that draws them.
pub use app::flow::{
    layout as flow_layout, FlowState, MAX_ZOOM as FLOW_MAX_ZOOM, MIN_ZOOM as FLOW_MIN_ZOOM,
};
pub use app::{
    AgentsTab, App, AppState, ChatScroll, ChatTab, InboxRow, MemberRow, Pane, PaneChrome,
    PaneSplits, PaneTab, ProseMeasure, ProseRow, RecordsTab, SidePanelTab, SideScope,
    WorkspaceMove, WorkspaceTarget,
};
pub use events::{UiCommand, UiEvent};
pub use mode_command::{ApprovalLevel, ModeCommandError, PermissionMode, SlashModeCommand};
/// Draws the whole UI into a ratatui frame; frontends other than
/// [`Terminal`] (the desktop app) call it with their own backend.
pub use renderer::render;
pub use terminal::Terminal;
pub use themes::Theme;

#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("render error: {0}")]
    Render(String),
    #[error("{0}")]
    Internal(String),
}

// Component re-exports
pub use components::{
    Activity, DiffLine, DiffView, HeaderBar, InputField, Message, MessageRole, ProgressBar,
    StatusBar, ThinkingStep, ToolCard, ToolStatus, ToolStep,
};

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
