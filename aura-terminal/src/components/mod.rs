//! UI components for the terminal interface.
//!
//! Each component is a self-contained rendering unit that can be composed
//! to build the full terminal UI.

mod diff;
mod header;
mod input;
mod message;
mod progress;
mod status;
mod tool_card;

pub use diff::{DiffLine, DiffLineType, DiffView};
pub use header::HeaderBar;
pub use input::InputField;
pub use message::{Message, MessageRole};
pub use progress::ProgressBar;
pub use status::StatusBar;
pub use tool_card::{ToolCard, ToolStatus};
