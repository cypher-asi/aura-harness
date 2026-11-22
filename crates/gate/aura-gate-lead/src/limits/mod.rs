//! What the lead may do itself while delegation is enforced:
//!
//! - `tools`: what it may use, every tool but the file-changing ones
//! - `commands`: what it may run, checks and inspection commands
//! - `caps`: how much of a read or a search it sees

mod caps;
mod commands;
mod tools;

pub(crate) use caps::{Cap, READ_LINES, SEARCH_HITS};
pub(crate) use commands::{allows_command, refused_command, RUN_COMMAND};
pub use tools::keeps_tool;
pub(crate) use tools::refused_tool;

#[cfg(test)]
mod tests;
