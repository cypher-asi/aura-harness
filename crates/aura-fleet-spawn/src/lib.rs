//! # aura-fleet-spawn
//!
//! Layer: fleet
//!
//! Spawn mechanics for subagents. Phase 7a's
//! [`FleetSpawner::spawn`] is the single composition seam that all
//! subagent-producing tools route through — replacing the legacy
//! coarse `spawn_lock` at `crates/aura-runtime/src/subagent_dispatch.rs`
//! and centralising:
//!
//! 1. [`AgentMode::allows_spawn`] gate (rejects `Plan`/`Ask`/`Debug`
//!    with a typed [`SpawnError::ModeViolation`]).
//! 2. Per-parent audit-append lease via [`ParentLeaseRegistry`] so
//!    concurrent spawns from the SAME parent serialise on the
//!    parent's append log while spawns from DIFFERENT parents run
//!    in parallel.
//! 3. `aura-agent-subagent::derive_subagent` invocation (depth +
//!    permission narrowing + mode narrowing validated here BEFORE
//!    any quota or audit work happens).
//! 4. Tracking-only [`aura_fleet_quota::QuotaPool::try_acquire`].
//! 5. Audit write of the [`SubagentSpawnRecordPayload`] (carrying
//!    the `OverrideManifest`) through
//!    [`aura_agent_kernel::write_system_record`] — the kernel is
//!    the ONLY writer of records, per Phase 6a invariant.
//! 6. [`FleetRegistry::register`] for the child slot.
//! 7. Dispatch to a pluggable [`ChildRunner`] for the actual
//!    child loop. Phase 7a exposes only [`SpawnMode::Wait`];
//!    [`SpawnMode::Detached`] and [`SpawnMode::Batch`] return
//!    [`SpawnError::NotImplementedInPhase7a`] so callers fail loudly.
//!
//! ## Invariants (per `.cursor/rules.md` §13)
//!
//! - **Order of operations**: mode-check → lease → derive → quota →
//!   audit write → registry insert → dispatch. Depth (checked inside
//!   derive) is therefore validated BEFORE quota acquisition; this
//!   ordering is fixed and asserted by
//!   `tests/depth_exceeded_before_quota.rs`.
//! - **Per-parent lease atomicity**: a single in-flight
//!   spawn-decision per parent at a time. Two spawn calls for the
//!   same parent serialise on the lease; the second call observes
//!   the first's audit-record write before issuing its own. The
//!   audit-log sequence numbers are therefore strictly monotone with
//!   no gaps under concurrent parent-side spawn calls.
//! - **Cross-parent parallelism**: unrelated parents own DISTINCT
//!   `Arc<Mutex<()>>` lease handles, so their spawn calls never
//!   block each other.
//! - **Kernel-only audit writes**: every `SubagentSpawn` audit
//!   record goes through [`aura_agent_kernel::write_system_record`].
//!   No parallel write path exists in this crate.
//! - **Closed-enum `SpawnMode` taxonomy**: Phase 7a accepts only
//!   [`SpawnMode::Wait`]. The other variants land in Phase 7b; until
//!   then they return [`SpawnError::NotImplementedInPhase7a`] so
//!   callers cannot silently fall through to `Wait`.
//!
//! ## Assumptions
//!
//! - The caller supplies a [`SpawnRequest`] containing a stable
//!   [`ParentContext`] snapshot — captured atomically by the
//!   spawning tool before any concurrent parent-state change can
//!   race the derivation.
//! - The [`ChildRunner`] implementation owns the runtime-side
//!   scheduling and identity registration; this crate does not
//!   reach into `aura-runtime` directly to preserve the
//!   layer-boundary contract.
//! - The pluggable [`SubagentDerivation`] defaults to
//!   [`aura_agent_subagent::DefaultDerivation`]; tests may swap a
//!   custom impl.
//!
//! ## Failure modes
//!
//! - [`SpawnError::ModeViolation`] — parent mode does not permit
//!   spawning. Fast-failed before any resource acquisition.
//! - [`SpawnError::Derivation`] — derivation rejected the
//!   overrides (depth exceeded, widening, etc.). Fast-failed
//!   before quota / audit / dispatch work.
//! - [`SpawnError::Quota`] — quota acquisition failed. Phase 7a's
//!   tracking-only impl never produces this variant; Phase 7b will.
//! - [`SpawnError::Audit`] — the kernel rejected the
//!   `SubagentSpawn` record append.
//! - [`SpawnError::Registry`] — `FleetRegistry::register` failed
//!   (e.g. duplicate `agent_id` — a logic bug).
//! - [`SpawnError::Child`] — the [`ChildRunner`] surfaced an error
//!   while running the child loop.
//! - [`SpawnError::NotImplementedInPhase7a`] — caller requested a
//!   [`SpawnMode`] this phase does not yet support.

#![forbid(unsafe_code)]
#![warn(clippy::all)]

mod lease;
mod runner;
mod spawner;

pub use lease::{ParentLease, ParentLeaseRegistry};
pub use runner::{ChildRunContext, ChildRunError, ChildRunner, TaskCompatContext};
pub use spawner::{
    FleetSpawner, FleetSpawnerConfig, SpawnError, SpawnHandle, SpawnRequest,
    SubagentSpawnRecordPayload, RECORD_KIND_SUBAGENT_SPAWN,
};

// Re-export the enums callers commonly need so they don't have to
// pull aura-core-modes / aura-agent-subagent transitively.
pub use aura_agent_subagent::{DerivationError, ParentContext, SubagentOverrides, SubagentSpec};
pub use aura_core_modes::{ModeViolation, SpawnMode};
