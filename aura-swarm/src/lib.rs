//! # aura-swarm
//!
//! Swarm runtime for Aura.
//!
//! Provides:
//! - HTTP router for transaction submission
//! - Scheduler for agent processing
//! - Per-agent worker loop with single-writer guarantee

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic, clippy::nursery)]

mod config;
pub mod protocol;
mod router;
mod scheduler;
pub mod session;
mod swarm;
mod worker;

pub use config::SwarmConfig;
pub use swarm::Swarm;
