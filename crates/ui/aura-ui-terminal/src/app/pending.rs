//! What waits on the user or the host: tool approvals, confirmations
//! asked for with `y`, and requests the event loop has not taken yet.

use std::collections::VecDeque;
use std::path::PathBuf;

use super::remote_files::PendingFile;
use super::{PendingApproval, WorkspaceMove};

/// Things waiting on the user or the host.
#[derive(Debug, Default)]
pub(crate) struct Pending {
    /// Calls waiting for an answer, oldest first; the front one is shown.
    /// Calls of one turn ask together, so one prompt must not replace
    /// another.
    pub(crate) approvals: VecDeque<PendingApproval>,
    /// Cloud model to select once the in-chat login completes
    pub(crate) model: Option<String>,
    /// Agent (hex id) whose queue the last key put a message in. The next
    /// key, if it is Enter on an empty prompt, sends that message now even
    /// before the event loop's `SetQueue` shows it, and whatever panel has
    /// the keys; any other key clears it.
    pub(crate) just_queued: Option<String>,
    /// File to open in the editor before the next frame
    pub(crate) editor_request: Option<PathBuf>,
    /// Temp file holding the input while Ctrl+G has it in the editor
    pub(crate) input_editor_file: Option<PathBuf>,
    /// Permanent agent waiting for `y` to confirm its deletion
    pub(crate) delete: Option<String>,
    /// Agent whose project waits for `y` to confirm removing it
    pub(crate) remove_project: Option<String>,
    /// Id of the channel waiting for `y` to be archived
    pub(crate) archive_channel: Option<String>,
    /// A move to another workspace the menu asked for, until the host
    /// takes it
    pub(crate) workspace_move: Option<WorkspaceMove>,
    /// Process waiting for `y` to confirm killing it
    pub(crate) kill: Option<String>,
    /// Node to select once the requested graph loads.
    pub(crate) graph_node: Option<String>,
    /// A file asked of the active agent's machine, shown once read
    pub(crate) file: Option<PendingFile>,
}
