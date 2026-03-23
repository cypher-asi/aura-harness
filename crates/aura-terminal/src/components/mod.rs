//! UI components for the terminal interface.
//!
//! Each component is a self-contained rendering unit that can be composed
//! to build the full terminal UI.

mod code_block;
mod diff;
mod header;
mod input;
mod message;
mod progress;
mod status;
mod tool_card;

pub use crate::events::MessageRole;
pub use code_block::CodeBlock;
pub use diff::{DiffLine, DiffLineType, DiffView};
pub use header::HeaderBar;
pub use input::InputField;
pub use message::Message;
pub use progress::ProgressBar;
pub use status::StatusBar;
pub use tool_card::{ToolCard, ToolStatus};
