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
    clippy::single_match,
    clippy::single_match_else,
    clippy::option_if_let_else,
    clippy::missing_panics_doc,
    clippy::needless_pass_by_value,
    clippy::unnecessary_map_or,
    clippy::wildcard_imports,
    clippy::manual_let_else,
    clippy::ignored_unit_patterns,
    clippy::significant_drop_tightening,
    clippy::missing_errors_doc,
    clippy::missing_const_for_fn,
    clippy::unused_self,
    clippy::struct_field_names
)]

pub(crate) mod automaton_bridge;
mod config;
pub(crate) mod domain;
pub(crate) mod executor_factory;
pub(crate) mod jwt_domain;
mod node;
pub(crate) mod protocol;
pub(crate) mod provider_factory;
pub(crate) mod router;
pub(crate) mod scheduler;
pub(crate) mod session;
pub(crate) mod terminal;
mod worker;

pub use config::NodeConfig;
pub use node::Node;

pub use aura_protocol::{
    ApprovalResponse, AssistantMessageEnd, AssistantMessageStart, ConversationMessage, ErrorMsg,
    FileOp, FilesChanged, InboundMessage, InstalledTool, OutboundMessage, SessionInit,
    SessionProviderConfig, SessionReady, SessionUsage, TextDelta, ThinkingDelta,
    ToolAuth as ProtocolToolAuth, ToolInfo, ToolResultMsg, ToolUseStart, UserMessage,
};

#[cfg(feature = "test-support")]
pub mod test_support {
    pub use crate::router::{create_router, RouterState};
    pub use crate::scheduler::Scheduler;
}

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
