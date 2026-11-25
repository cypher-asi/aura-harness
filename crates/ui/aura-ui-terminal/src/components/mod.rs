//! UI components for the terminal interface.
//!
//! Each component is a self-contained rendering unit that can be composed
//! to build the full terminal UI.

mod activity;
mod boot;
mod code_block;
mod diff;
mod header;
mod input;
mod live_output;
mod message;
mod progress;
mod status;
pub(crate) mod syntax;
mod tool_card;

pub use crate::events::MessageRole;
pub(crate) use activity::elapsed_ms;
pub use activity::{Activity, ThinkingStep, ToolStep};
pub use boot::{BootConsole, BootOutcome, BootStep, MAX_BOOT_LOG_LINES};
pub use code_block::{highlight_code_line, CodeBlock};
pub(crate) use code_block::{unframe_code, COPY_BUTTON};
pub use diff::{DiffLine, DiffLineType, DiffView};
pub use header::HeaderBar;
pub use input::InputField;
pub use live_output::{quiet_for, LiveOutput, QUIET_AFTER};
pub use message::{DiffBlock, Message, ShellRun};
pub use progress::ProgressBar;
pub use status::StatusBar;
pub use tool_card::{ToolCard, ToolStatus};
