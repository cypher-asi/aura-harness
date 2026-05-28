//! [`FleetSpawner`] — Phase 7a's single subagent spawn entrypoint.
//!
//! See the crate-level docs for the full invariant + ordering
//! contract. This module wires the pieces together.

use std::sync::Arc;

use aura_agent_kernel::write_system_record;
use aura_agent_subagent::{
    DefaultDerivation, DerivationError, OverrideManifest, ParentContext, SubagentDerivation,
    SubagentOverrides,
};
use aura_core::{AgentId, SubagentResult, Transaction, TransactionType};
use aura_core_modes::{ModeViolation, SpawnMode};
use aura_fleet_quota::{QuotaError, QuotaPool, QuotaRequest};
use aura_fleet_registry::{AgentSlot, FleetRegistry, RegistryError};
use aura_store::Store;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, instrument, warn};

use crate::lease::ParentLeaseRegistry;
use crate::runner::{ChildRunContext, ChildRunError, ChildRunner, TaskCompatContext};

/// Stable kind tag stamped on the JSON envelope written for every
/// successful spawn. Mirrors the Phase 7+ `RecordKind::SubagentSpawn`
/// taxonomy from `aura-store-record`. Today the on-disk record
/// `tx_type` is [`TransactionType::System`] so the envelope's `kind`
/// field is the canonical discriminator until the layered record
/// schema lands.
pub const RECORD_KIND_SUBAGENT_SPAWN: &str = "subagent_spawn";

/// Wire shape of the `SubagentSpawn` audit record's payload.
///
/// Phase 7a writes this through
/// [`aura_agent_kernel::write_system_record`] using
/// [`TransactionType::System`]. Phase 6+'s layered
/// `RecordKind::SubagentSpawn` will move the `kind` discriminator
/// out of the envelope and into the record header proper without
/// changing the body fields, so this struct is forward-compatible
/// with the eventual schema migration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubagentSpawnRecordPayload {
    /// Discriminator for the System-record envelope (`"subagent_spawn"`).
    pub kind: String,
    /// Parent agent that requested the spawn.
    pub parent_agent_id: AgentId,
    /// Freshly-allocated child agent id.
    pub child_agent_id: AgentId,
    /// Manifest of explicit overrides the parent supplied. May be
    /// empty when the child inherits every field — see
    /// [`OverrideManifest::is_empty`].
    pub override_manifest: OverrideManifest,
}

/// Request handed to [`FleetSpawner::spawn`].
#[derive(Debug, Clone)]
pub struct SpawnRequest {
    /// Atomic snapshot of the parent's session state at spawn time.
    pub parent: ParentContext,
    /// Explicit overrides the parent's tool call supplied; all
    /// `Option<_>` so unset fields inherit from the parent.
    pub overrides: SubagentOverrides,
    /// Initial prompt seeded into the child agent loop.
    pub prompt: String,
    /// Originating user id propagated through to the child's
    /// audit attribution + scheduler identity.
    pub originating_user_id: Option<String>,
    /// Phase 7a-only compatibility carrier. Forwarded to the
    /// [`ChildRunner`] so the legacy task-tool path can read the
    /// fields not yet modelled on [`SubagentSpec`]. Phase 7b
    /// retires this field once
    /// [`aura_agent_subagent::SubagentOverrides`] grows the full
    /// override surface.
    pub task_compat: Option<TaskCompatContext>,
}

/// Outcome of a [`FleetSpawner::spawn`] call.
///
/// Phase 7a only exposes [`SpawnHandle::Completed`] because the
/// only legal [`SpawnMode`] is [`SpawnMode::Wait`]. Phase 7b adds
/// `Pending { child_agent_id }` for `Detached` and a batch handle.
#[derive(Debug)]
pub enum SpawnHandle {
    /// `SpawnMode::Wait` ran the child loop to completion in the
    /// current task; the [`SubagentResult`] is the byte-identical
    /// shape the task tool returns to the parent agent.
    Completed(SubagentResult),
}

/// Errors returned by [`FleetSpawner::spawn`].
#[derive(Debug, Error)]
pub enum SpawnError {
    /// Parent mode does not permit spawning (Plan/Ask/Debug).
    /// Fast-failed before any resource acquisition.
    #[error("spawn rejected: parent mode disallows spawning ({0})")]
    ModeViolation(#[from] ModeViolation),

    /// `aura-agent-subagent::derive_subagent` rejected the request
    /// (depth exceeded, mode/permission widening, etc.).
    #[error("spawn rejected by derivation: {0}")]
    Derivation(#[from] DerivationError),

    /// Quota acquisition failed. Phase 7a's tracking-only pool
    /// never produces this; reserved for Phase 7b enforcement.
    #[error("spawn rejected by quota: {0}")]
    Quota(#[from] QuotaError),

    /// `aura-agent-kernel::write_system_record` failed.
    #[error("spawn rejected by audit kernel: {0}")]
    Audit(String),

    /// `FleetRegistry::register` failed (e.g. duplicate id).
    #[error("spawn rejected by registry: {0}")]
    Registry(#[from] RegistryError),

    /// The pluggable child runner errored.
    #[error("spawn child runner failed: {0}")]
    Child(#[from] ChildRunError),

    /// Caller asked for a [`SpawnMode`] Phase 7a does not yet
    /// expose. Phase 7b owns `Detached` / `Batch`.
    #[error("spawn mode {0:?} is not implemented in Phase 7a; reserved for Phase 7b")]
    NotImplementedInPhase7a(SpawnMode),

    /// Serde failure assembling the `SubagentSpawn` audit payload.
    /// `serde_json` can only fail here on bytes that fail to UTF-8
    /// encode (impossible for our shape) or on a custom serialize
    /// impl — both indicate a logic bug.
    #[error("spawn audit payload serialization failed: {0}")]
    Serialization(String),
}

/// Construction config for [`FleetSpawner`].
#[derive(Debug, Clone)]
pub struct FleetSpawnerConfig {
    /// Quota request shape applied to every spawn. Phase 7a wires
    /// this from the resolved [`SubagentSpec::budget`].
    pub max_concurrent_tools: u32,
}

impl Default for FleetSpawnerConfig {
    fn default() -> Self {
        Self {
            max_concurrent_tools: 4,
        }
    }
}

/// Composition root for subagent spawn. See the crate-level docs
/// for the per-spawn ordering contract.
pub struct FleetSpawner {
    store: Arc<dyn Store>,
    registry: Arc<FleetRegistry>,
    quota: Arc<QuotaPool>,
    leases: Arc<ParentLeaseRegistry>,
    derivation: Arc<dyn SubagentDerivation>,
    child_runner: Arc<dyn ChildRunner>,
    config: FleetSpawnerConfig,
}

impl FleetSpawner {
    /// Construct a [`FleetSpawner`] with a custom derivation.
    /// Use [`Self::with_default_derivation`] for the standard
    /// [`DefaultDerivation`] config.
    #[must_use]
    pub fn new(
        store: Arc<dyn Store>,
        registry: Arc<FleetRegistry>,
        quota: Arc<QuotaPool>,
        leases: Arc<ParentLeaseRegistry>,
        derivation: Arc<dyn SubagentDerivation>,
        child_runner: Arc<dyn ChildRunner>,
        config: FleetSpawnerConfig,
    ) -> Self {
        Self {
            store,
            registry,
            quota,
            leases,
            derivation,
            child_runner,
            config,
        }
    }

    /// Construct a [`FleetSpawner`] using the bundled
    /// [`DefaultDerivation`].
    #[must_use]
    pub fn with_default_derivation(
        store: Arc<dyn Store>,
        registry: Arc<FleetRegistry>,
        quota: Arc<QuotaPool>,
        leases: Arc<ParentLeaseRegistry>,
        child_runner: Arc<dyn ChildRunner>,
        config: FleetSpawnerConfig,
    ) -> Self {
        Self::new(
            store,
            registry,
            quota,
            leases,
            Arc::new(DefaultDerivation::default()),
            child_runner,
            config,
        )
    }

    /// Spawn a subagent. See crate-level docs for the full
    /// ordering contract.
    ///
    /// # Errors
    ///
    /// Returns one of the [`SpawnError`] variants documented at
    /// the crate root.
    #[instrument(
        skip(self, request),
        fields(parent_agent_id = %request.parent.agent_id, mode = ?mode)
    )]
    pub async fn spawn(
        &self,
        request: SpawnRequest,
        mode: SpawnMode,
    ) -> Result<SpawnHandle, SpawnError> {
        // (0) SpawnMode taxonomy gate — Phase 7a only supports
        //     `Wait`. Reject the others hard so callers cannot
        //     silently fall through to a different semantics than
        //     they asked for.
        match mode {
            SpawnMode::Wait => {}
            SpawnMode::Detached | SpawnMode::Batch => {
                warn!(
                    requested = ?mode,
                    "fleet spawner: SpawnMode reserved for Phase 7b — refusing"
                );
                return Err(SpawnError::NotImplementedInPhase7a(mode));
            }
        }

        // (1) Parent-mode gate. Plan/Ask/Debug short-circuit before
        //     any state touches the lease, quota, or audit log.
        if !request.parent.mode.allows_spawn() {
            debug!(
                parent_mode = ?request.parent.mode,
                "fleet spawner: parent mode forbids spawning"
            );
            return Err(SpawnError::ModeViolation(ModeViolation::SpawnNotAllowed));
        }

        let parent_agent_id = request.parent.agent_id;

        // (2) Per-parent audit-append lease — held across every
        //     remaining step so two concurrent spawns from one
        //     parent serialise their audit-record appends.
        let _lease = self.leases.acquire(parent_agent_id).await;
        debug!("fleet spawner: lease acquired");

        // (3) Derivation — runs depth + mode + permission validation
        //     and produces the canonical SubagentSpec. Depth is
        //     checked FIRST inside derive so a too-deep spawn never
        //     reaches step 4 (quota).
        let (spec, manifest) = self
            .derivation
            .derive(&request.parent, request.overrides.clone())?;

        debug!(
            child_agent_id_candidate = %spec.parent,
            depth = spec.depth,
            kernel_mode = ?spec.kernel_mode,
            override_count = manifest.applied.len(),
            "fleet spawner: spec derived"
        );

        // (4) Tracking-only quota acquire. The ticket is not yet
        //     enforced (Phase 7b) but the call ordering is fixed:
        //     `try_acquire` runs AFTER `derive_subagent` so a
        //     depth-rejected spawn never consumes a ticket.
        let _ticket = self.quota.try_acquire(QuotaRequest {
            agent_id: parent_agent_id,
            max_iterations: spec.budget.max_iterations,
            max_concurrent_tools: self.config.max_concurrent_tools,
            token_budget: Some(u64::from(spec.budget.max_tokens)),
        })?;

        // (5) The child id is currently the inner ChildRunner's
        //     responsibility (the legacy KernelSpawnHook allocates
        //     fresh ids inside `spawn_child`). Phase 7a writes the
        //     `SubagentSpawn` audit record WITHOUT the child id
        //     (a `nil`-AgentId placeholder) because we cannot peek
        //     at the future child id without running the runner
        //     first, and the runner ALSO needs to write child-side
        //     records that must happen UNDER the same lease. The
        //     reverse ordering would leak a half-recorded spawn if
        //     the runner failed.
        //
        //     Phase 7b moves child-id allocation into this crate so
        //     the audit record carries the final id; that change is
        //     orthogonal to the per-parent lease invariant which
        //     this crate already enforces.
        let manifest_payload = SubagentSpawnRecordPayload {
            kind: RECORD_KIND_SUBAGENT_SPAWN.to_string(),
            parent_agent_id,
            child_agent_id: AgentId::default(),
            override_manifest: manifest.clone(),
        };
        let manifest_bytes = serde_json::to_vec(&manifest_payload)
            .map_err(|e| SpawnError::Serialization(format!("manifest: {e}")))?;
        let audit_tx = Transaction::new_chained(
            parent_agent_id,
            TransactionType::System,
            Bytes::from(manifest_bytes),
            None,
        );
        let seq = write_system_record(&self.store, parent_agent_id, audit_tx)
            .map_err(|e| SpawnError::Audit(e.to_string()))?;
        info!(
            seq,
            override_count = manifest.applied.len(),
            "fleet spawner: SubagentSpawn audit record appended"
        );

        // (6) Dispatch the child loop. Wait mode runs to completion
        //     synchronously so we can return the byte-identical
        //     SubagentResult to the parent tool call.
        let result = self
            .child_runner
            .run(ChildRunContext {
                spec: spec.clone(),
                prompt: request.prompt,
                originating_user_id: request.originating_user_id,
                task_compat: request.task_compat,
            })
            .await?;

        // (7) Registry insertion happens AFTER the runner so we can
        //     use the runner-allocated child id from the
        //     SubagentResult. The runner produces `None` only when
        //     it short-circuits before child creation (e.g. unknown
        //     kind); we skip registry for that case since there is
        //     no child to track.
        if let Some(child_agent_id) = result.child_agent_id {
            let slot = AgentSlot::new(
                child_agent_id,
                Some(parent_agent_id),
                spec.mode,
                spec.kernel_mode,
                spec.permissions.clone(),
            );
            self.registry.register(slot)?;
        }

        Ok(SpawnHandle::Completed(result))
    }
}
