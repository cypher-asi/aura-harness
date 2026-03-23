//! # aura-node
//!
//! Node runtime for Aura.
//!
//! Provides:
//! - HTTP router for transaction submission
//! - Scheduler for agent processing
//! - Per-agent worker loop with single-writer guarantee

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic, clippy::nursery)]
#![allow(
    clippy::doc_markdown,
    clippy::must_use_candidate,
    clippy::match_same_arms,
    clippy::too_many_lines,
    clippy::single_match,
    clippy::single_match_else,
    clippy::option_if_let_else,
    clippy::missing_panics_doc,
    clippy::needless_pass_by_value,
    clippy::unnecessary_map_or
)]

mod config;
mod node;
pub mod protocol;
mod router;
mod scheduler;
pub mod session;
mod worker;

pub use config::NodeConfig;
pub use node::Node;

/// Top-level error type for the aura-node crate.
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    /// Server bind or runtime error.
    #[error("server error: {0}")]
    Server(#[from] std::io::Error),

    /// Storage layer failure.
    #[error("store error: {0}")]
    Store(#[from] anyhow::Error),

    /// Address parse failure.
    #[error("invalid bind address: {0}")]
    InvalidAddress(#[from] std::net::AddrParseError),
}
