//! How the agent's todo list is shown: above the input, and in the chat's
//! live checklist.

/// Whether the todo list is open and the checklist expanded.
#[derive(Debug, Default)]
pub(crate) struct TodoView {
    /// Whether the full todo list is shown above the input
    pub(crate) open: bool,
    /// Whether the chat's live to-do checklist lists every item in full
    pub(crate) expanded: bool,
}
