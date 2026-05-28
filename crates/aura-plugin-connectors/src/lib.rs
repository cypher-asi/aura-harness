//! # aura-plugin-connectors
//!
//! Layer: plugin
//!
//! Connector registry for plugin-contributed external endpoints.
//! Phase 4c shipped registration + lookup; later phases materialise
//! enabled, trusted manifest contributions into the registry.
//!
//! ## Surfaces
//!
//! - [`ConnectorEntry`] — opaque registry value carrying the
//!   connector id, contributing plugin id, and endpoint string.
//! - [`ConnectorRegistry`] — thread-safe in-process registry.
//!   `register` preserves first-contributor-wins by returning
//!   [`ConnectorError::AlreadyRegistered`]; `replace` is the
//!   runtime materialiser's last-wins API for plugin overrides.
//! - Error type: [`ConnectorError`].
//!
//! ## Invariants ([rules.md §13])
//!
//! - The registry uses a `BTreeMap` so [`ConnectorRegistry::list`]
//!   returns entries in deterministic id-sorted order — important
//!   for diagnostics (`aura plugins list` derivatives) and for
//!   future replay-determinism tests.
//! - Connector ids are global across the entire registry.
//!   Callers choose merge policy explicitly: [`ConnectorRegistry::register`]
//!   rejects duplicates, while [`ConnectorRegistry::replace`] overwrites
//!   an existing entry and returns the displaced connector for audit logs.

#![forbid(unsafe_code)]
#![warn(clippy::all)]

pub mod error;
pub mod registry;

pub use error::ConnectorError;
pub use registry::{ConnectorEntry, ConnectorRegistry};
