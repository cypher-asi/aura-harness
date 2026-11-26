//! # aura-engine-registry
//!
//! Layer: engine
//!
//! The persistent list of the projects and agents the terminal UI and the
//! desktop app manage, and the permanent agent definitions (named roles)
//! that agents are spawned from.
//!
//! ## Surface
//!
//! - [`registry`]: [`AgentRegistry`], stored in the `agents`, `projects`
//!   and `registry_meta` column families of the agent-state database at
//!   `<data_dir>/db`, one row per agent and per project.
//! - [`definitions`]: [`AgentDefinitions`], the Markdown definitions in
//!   `<data_dir>/agent_defs/`, re-read before every turn.
//!
//! `aura-app` re-exports these modules as `aura_app::agent_registry` and
//! `aura_app::agent_definitions`.

#![forbid(unsafe_code)]

pub mod definitions;
mod error;
pub mod registry;

pub use definitions::{AgentDefinition, AgentDefinitions};
pub use error::{AgentDefinitionError, AgentRegistryError};
pub use registry::{AgentRegistry, Opened, Project, RegisteredAgent};
