//! Filesystem tool implementations.

mod cmd;
mod delete;
mod edit;
mod find;
mod ls;
mod read;
mod search;
mod stat;
mod write;

pub use cmd::{
    cmd_run, cmd_run_with_threshold, cmd_spawn, output_to_tool_result, CmdRunTool,
    ThresholdResult,
};
pub use delete::{fs_delete, FsDeleteTool};
pub use edit::{fs_edit, FsEditTool};
pub use find::{fs_find, FsFindTool};
pub use ls::{fs_ls, FsLsTool};
pub use read::{fs_read, FsReadTool};
pub use search::{search_code, SearchCodeTool};
pub use stat::{fs_stat, FsStatTool};
pub use write::{fs_write, FsWriteTool};

use crate::error::ToolError;
use aura_core::ToolResult;

/// Run a blocking tool closure on the tokio blocking threadpool.
pub(crate) async fn spawn_blocking_tool<F>(f: F) -> Result<ToolResult, ToolError>
where
    F: FnOnce() -> Result<ToolResult, ToolError> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ToolError::CommandFailed(format!("blocking task panicked: {e}")))?
}
