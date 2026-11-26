//! Session checkpoints for the terminal: the shared checkpoint files of
//! [`aura_agent::checkpoint`], which a cloud machine's node writes too.

pub use aura_agent::checkpoint::{
    is_resumable, now_ms, Checkpoints, SessionCheckpoint, SessionListing, SessionUsage,
};
