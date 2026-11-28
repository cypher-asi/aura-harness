//! View and summary types the commands and events carry, one file per
//! domain. Everything is re-exported from [`crate::events`].

pub(super) mod agents;
pub(super) mod boot;
pub(super) mod channels;
pub(super) mod hud;
pub(super) mod mail;
pub(super) mod merge_queue;
pub(super) mod messages;
pub(super) mod plans;
pub(super) mod policies;
pub(super) mod processes;
pub(super) mod records;
pub(super) mod summaries;
pub(super) mod trace;
pub(super) mod usage;
