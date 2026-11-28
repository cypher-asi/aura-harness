//! UI components for the terminal interface.
//!
//! Each component is a self-contained rendering unit that can be composed
//! to build the full terminal UI.

mod activity;
mod agent_row;
mod boot;
mod code_block;
mod detail_row;
mod diff;
mod header;
pub mod image_preview;
mod info;
mod input;
mod list_row;
mod live_output;
mod message;
mod modal;
mod progress;
mod relative_time;
mod status;
mod status_dot;
pub mod syntax;
mod tool_card;

pub use activity::elapsed_ms;
pub use activity::{Activity, ThinkingStep, ToolStep};
pub use agent_row::{agent_button_at, agent_button_offset, AgentButton, AgentRowEnd};
pub use aura_ui::events::MessageRole;
pub use boot::{
    boot_duration, wait_clock, BootConsole, BootOutcome, BootStep, BootWait, LIVE_BOOT_LOG_LINES,
    MAX_BOOT_LOG_LINES,
};
pub use code_block::{highlight_code_line, CodeBlock};
pub use code_block::{unframe_code, COPY_BUTTON};
pub use detail_row::detail_row;
pub use diff::{DiffLine, DiffLineType, DiffView};
pub use header::HeaderBar;
pub use info::{truncate_line, Info, Value};
pub use input::InputField;
pub use list_row::{
    cell_style, format_ago, list_row, list_row_ending, list_row_with_detail, tree_row,
    truncate_record_info, CellState, RowBadge, StatusIcon, PLAN_BADGE, ROW_INSET,
};
pub use live_output::{quiet_for, LiveOutput, QUIET_AFTER};
pub use message::{DiffBlock, Message, MoreSection, ShellRun, MORE_PAGE_LINES};
pub use modal::{
    backdrop, caret_line, choice_line, list_item, popup_block, ListScroll, Modal, ModalArea, Tone,
    STANDARD_WIDTH, WIDE_WIDTH,
};
pub use progress::ProgressBar;
pub use relative_time::{exact_time, parse_time, relative_time, relative_time_changes};
pub use status::StatusBar;
pub use status_dot::StatusDot;
pub use tool_card::{ToolCard, ToolStatus};
