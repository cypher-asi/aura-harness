//! # aura-fleet-daemon
//!
//! Layer: fleet
//!
//! Composition root for the fleet layer. Phase 7a wires
//! [`FleetRegistry`], [`FleetSpawner`], [`FleetDispatcher`], and
//! [`QuotaPool`] into a single [`FleetDaemon`] holder so the surface
//! crates (and today's `aura-runtime` adapter) can grab an
//! `Arc<FleetDaemon>` and reach into any of the four subsystems
//! through [`FleetDaemon::handle`].
//!
//! Phase 7b grows this crate substantially: it will own the event
//! loop, the mailbox router, the plugin materialisation seam, and
//! the session supervisor. Phase 7a's [`FleetDaemon::run`] is a
//! deliberate no-op shell — calling it is harmless and the daemon
//! becomes useful through the typed handle accessors.
//!
//! ## Invariants (per `.cursor/rules.md` §13)
//!
//! - All Arc handles are constructed inside
//!   [`FleetDaemon::builder`] / [`FleetDaemon::new`] so there is a
//!   single deterministic source for each subsystem. Surface crates
//!   may NOT new up their own [`FleetSpawner`] / [`FleetRegistry`] /
//!   etc. — that would defeat the cross-spawn invariants (e.g. a
//!   second [`FleetSpawner`] with its own [`ParentLeaseRegistry`]
//!   would not serialise spawns from the same parent against the
//!   first spawner's leases).
//! - Construction is failable only via the optional `try_*`
//!   helpers Phase 7b adds; Phase 7a's `new` cannot fail since it
//!   does no I/O.
//!
//! ## Assumptions
//!
//! - The caller owns a `tokio::runtime::Runtime` (library crates
//!   never construct one — see plan §2 cross-cutting ownership).
//! - The caller supplies a concrete `Arc<dyn Store>` and a concrete
//!   `Arc<dyn ChildRunner>` because both ultimately live in the
//!   surface / runtime layer and the daemon does not know how to
//!   build either itself.
//!
//! ## Failure modes
//!
//! - [`FleetDaemonError::NoOpShell`] — placeholder variant kept
//!   non-empty so the closed-enum gains forward-compat headroom
//!   without churn when Phase 7b adds real variants.

#![forbid(unsafe_code)]
#![warn(clippy::all)]

use std::sync::Arc;

use aura_fleet_dispatch::FleetDispatcher;
use aura_fleet_quota::QuotaPool;
use aura_fleet_registry::FleetRegistry;
use aura_fleet_spawn::{ChildRunner, FleetSpawner, FleetSpawnerConfig, ParentLeaseRegistry};
use aura_store::Store;
use thiserror::Error;
use tracing::info;

pub use aura_fleet_dispatch::AgentJob;

/// Wiring config consumed at daemon construction time.
#[derive(Debug, Clone, Default)]
pub struct DaemonConfig {
    /// Per-spawn quota request shape. Phase 7a's pool is
    /// tracking-only so these values are recorded but not yet
    /// enforced.
    pub spawner: FleetSpawnerConfig,
}

/// Errors surfaced by [`FleetDaemon`] APIs. Phase 7a has no
/// terminal variants; the enum exists for forward-compat.
#[derive(Debug, Error)]
pub enum FleetDaemonError {
    /// Reserved — placeholder so the closed enum compiles even
    /// before Phase 7b adds real variants. Never returned.
    #[error("fleet daemon: phase-7a no-op shell error variant (never returned)")]
    NoOpShell,
}

/// Read-only bundle of [`Arc`] handles to the daemon's subsystems.
/// Cheap to clone — each field is a single `Arc`.
#[derive(Clone)]
pub struct FleetDaemonHandle {
    registry: Arc<FleetRegistry>,
    spawner: Arc<FleetSpawner>,
    dispatcher: Arc<FleetDispatcher>,
    quota: Arc<QuotaPool>,
    leases: Arc<ParentLeaseRegistry>,
}

impl FleetDaemonHandle {
    /// Shared [`FleetRegistry`] handle.
    #[must_use]
    pub fn registry(&self) -> Arc<FleetRegistry> {
        self.registry.clone()
    }

    /// Shared [`FleetSpawner`] handle.
    #[must_use]
    pub fn spawner(&self) -> Arc<FleetSpawner> {
        self.spawner.clone()
    }

    /// Shared [`FleetDispatcher`] handle.
    #[must_use]
    pub fn dispatcher(&self) -> Arc<FleetDispatcher> {
        self.dispatcher.clone()
    }

    /// Shared [`QuotaPool`] handle.
    #[must_use]
    pub fn quota(&self) -> Arc<QuotaPool> {
        self.quota.clone()
    }

    /// Shared [`ParentLeaseRegistry`] handle. Surface crates that
    /// need to peek at the lease pool for observability use this;
    /// the actual acquire/release flow stays inside
    /// [`FleetSpawner`].
    #[must_use]
    pub fn leases(&self) -> Arc<ParentLeaseRegistry> {
        self.leases.clone()
    }
}

/// Phase 7a composition root. Holds owned `Arc`s to the four
/// fleet subsystems plus the shared dispatcher; surface code reads
/// them via [`FleetDaemon::handle`].
pub struct FleetDaemon {
    handle: FleetDaemonHandle,
    config: DaemonConfig,
}

impl FleetDaemon {
    /// Construct a fully-wired daemon.
    ///
    /// The caller supplies the store + child runner because both
    /// types are runtime-side (the daemon crate cannot synthesise
    /// either without depending upward on `aura-runtime`).
    #[must_use]
    pub fn new(
        store: Arc<dyn Store>,
        child_runner: Arc<dyn ChildRunner>,
        config: DaemonConfig,
    ) -> Self {
        let registry = Arc::new(FleetRegistry::new());
        let quota = Arc::new(QuotaPool::new());
        let leases = Arc::new(ParentLeaseRegistry::new());
        let spawner = Arc::new(FleetSpawner::with_default_derivation(
            store,
            registry.clone(),
            quota.clone(),
            leases.clone(),
            child_runner,
            config.spawner.clone(),
        ));
        let dispatcher = Arc::new(FleetDispatcher::new(spawner.clone()));
        info!("fleet daemon: subsystems wired (Phase 7a)");
        Self {
            handle: FleetDaemonHandle {
                registry,
                spawner,
                dispatcher,
                quota,
                leases,
            },
            config,
        }
    }

    /// Cheap-clone handle to the daemon's subsystems.
    #[must_use]
    pub fn handle(&self) -> FleetDaemonHandle {
        self.handle.clone()
    }

    /// Read-only access to the resolved daemon config.
    #[must_use]
    pub fn config(&self) -> &DaemonConfig {
        &self.config
    }

    /// Phase 7a no-op event loop placeholder. Phase 7b replaces
    /// this with the real daemon lifecycle (mailbox routing,
    /// graceful shutdown, plugin reload). Calling it today is
    /// harmless and resolves immediately.
    ///
    /// # Errors
    ///
    /// Never errors in Phase 7a — the [`Result`] shape is kept so
    /// the Phase 7b promotion doesn't break callers.
    pub fn run(&self) -> Result<(), FleetDaemonError> {
        info!("fleet daemon: Phase 7a `run` is a no-op shell — Phase 7b owns the event loop");
        Ok(())
    }
}
