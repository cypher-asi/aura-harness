//! Layer: gate
//!
//! What the lead agent under Gate may do itself.
//!
//! Under Gate the main agent is a lead: it reads enough to decide, hands
//! the work to subtasks Gate routes to cheaper models, and integrates
//! their results. While delegation is enforced it has no file-changing
//! tools ([`keeps_tool`]), reads and searches come back capped so heavy
//! reading goes to an explore subtask, and `run_command` only runs
//! checks, linters in check mode and inspection commands. Subagents are
//! never restricted this way.
//!
//! - `way`: whether a turn is led or done by its model ([`choose_way`])
//! - `limits`: what the lead may use, run and read ([`keeps_tool`])
//! - `executor`: a led turn's tools and calls ([`lead_tools`], [`LeadExecutor`])
//! - [`delegation`]: how much of a lead's work went to subtasks

pub mod delegation;
mod executor;
mod limits;
mod way;

pub use executor::{lead_tools, LeadExecutor};
pub use limits::keeps_tool;
pub use way::{choose_way, Way};
