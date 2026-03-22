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

pub use cmd::{cmd_run_with_threshold, cmd_spawn, output_to_tool_result, CmdRunTool, ThresholdResult};
pub use delete::FsDeleteTool;
pub use edit::FsEditTool;
pub use find::FsFindTool;
pub use ls::FsLsTool;
pub use read::FsReadTool;
pub use search::SearchCodeTool;
pub use stat::FsStatTool;
pub use write::FsWriteTool;

use crate::error::ToolError;
use aura_core::ToolResult;

/// Run a blocking tool closure on the tokio blocking threadpool.
pub async fn spawn_blocking_tool<F>(f: F) -> Result<ToolResult, ToolError>
where
    F: FnOnce() -> Result<ToolResult, ToolError> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ToolError::CommandFailed(format!("blocking task panicked: {e}")))?
}
