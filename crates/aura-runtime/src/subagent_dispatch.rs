//! Foreground `task` subagent dispatch — Phase 7a routes through
//! the fleet layer.
//!
//! ## Phase 7a layering
//!
//! Before Phase 7a this file owned the entire spawn pipeline:
//!
//! - per-process `spawn_lock: Mutex<()>` (a single serialisation
//!   point used by *every* parent),
//! - direct `KernelSpawnHook::spawn_child` invocation,
//! - direct scheduler / identity-registry / store writes,
//! - terminal-state translation into [`SubagentResult`].
//!
//! Phase 7a deletes the legacy structure: derivation moved into
//! `aura-agent-subagent::DefaultDerivation`, the global lock became
//! `aura-fleet-spawn::ParentLeaseRegistry` (per-parent so unrelated
//! parents stop blocking each other), the audit record write moved
//! behind `aura-agent-kernel::write_system_record`, and the
//! `AgentSlot` is recorded in `aura-fleet-registry`.
//!
//! What lives in this file now:
//!
//! - [`RuntimeChildRunner`] — implements the
//!   [`aura_fleet_spawn::ChildRunner`] trait. Receives a
//!   [`aura_fleet_spawn::ChildRunContext`] (with the Phase 7a
//!   [`aura_fleet_spawn::TaskCompatContext`] compatibility carrier)
//!   and runs the byte-identical legacy schedule / identity / agent-
//!   loop path so the [`SubagentResult`] returned to the parent
//!   `task` tool stays stable across the refactor.
//! - [`RuntimeSubagentDispatch`] — implements the public
//!   [`SubagentDispatchHook`] surface. Builds a [`SpawnRequest`]
//!   from the legacy [`SubagentDispatchRequest`] and delegates to
//!   `FleetSpawner::spawn(..., SpawnMode::Wait)`.
//!
//! ## Compatibility invariants
//!
//! - The `SubagentResult` JSON shape returned through the task tool
//!   is byte-identical to the pre-Phase-7a output. Pinned by
//!   `crates/aura-runtime/tests/task_tool_subagent_result_snapshot.rs`.
//! - `RuntimeChildRunner` performs the same parent-permission
//!   narrowing, per-tool permission resolution, child identity
//!   registration, and scheduler dispatch as the deleted legacy
//!   implementation.
//! - Phase 7b is expected to retire [`TaskCompatContext`] once
//!   `aura-agent-subagent::SubagentOverrides` grows the
//!   `subagent_type`, `system_prompt_addendum`,
//!   `parent_tool_permissions`, and `user_tool_defaults` fields as
//!   first-class override knobs.

use crate::scheduler::Scheduler;
use crate::subagent_registry::SubagentRegistry;
use async_trait::async_trait;
use aura_agent::AgentLoopConfig;
use aura_agent_subagent::{ParentContext, SubagentLineage, SubagentOverrides};
use aura_core::{
    resolve_effective_permission, AgentPermissions, AgentToolPermissions, SubagentDispatchRequest,
    SubagentExit, SubagentKindSpec, SubagentResult, ToolState, Transaction, TransactionType,
    UserDefaultMode, UserToolDefaults,
};
use aura_core_modes::{AgentMode, KernelMode, ModeProfile, ReplayMode, SandboxMode, SpawnMode};
use aura_core_permissions::Permissions;
use aura_fleet_quota::QuotaPool;
use aura_fleet_registry::FleetRegistry;
use aura_fleet_spawn::{
    ChildRunContext, ChildRunError, ChildRunner, FleetSpawner, FleetSpawnerConfig,
    ParentLeaseRegistry, SpawnError, SpawnHandle, SpawnRequest, TaskCompatContext,
};
use aura_kernel::PolicyConfig;
use aura_store::Store;
use aura_tools::SubagentDispatchHook;
use bytes::Bytes;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

/// Foreground `task` dispatcher backed by the fleet layer.
///
/// Replaces the pre-Phase-7a in-runtime spawn pipeline with a thin
/// adapter on top of [`FleetSpawner`]:
///
/// 1. Translates the legacy [`SubagentDispatchRequest`] into a
///    [`ParentContext`] + [`SubagentOverrides`] + Phase 7a
///    [`TaskCompatContext`] tuple.
/// 2. Calls [`FleetSpawner::spawn`] in [`SpawnMode::Wait`] — the
///    only spawn mode Phase 7a wires.
/// 3. Maps the returned [`SpawnHandle::Completed`] back into the
///    byte-identical [`SubagentResult`] the task tool expects.
///
/// Lifetime + concurrency: this struct is cheap to clone (everything
/// behind `Arc`) and is shared across the executor / resolver chain.
pub struct RuntimeSubagentDispatch {
    /// Bundled subagent kind registry — read on every dispatch to
    /// resolve the requested `subagent_type` into a
    /// [`SubagentKindSpec`].
    registry: SubagentRegistry,
    /// Shared fleet spawner. Owns the per-parent
    /// [`ParentLeaseRegistry`] + [`FleetRegistry`] + derivation +
    /// audit-write pipeline.
    spawner: Arc<FleetSpawner>,
}

impl std::fmt::Debug for RuntimeSubagentDispatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeSubagentDispatch")
            .field("registry_kinds", &self.registry.all().len())
            .finish()
    }
}

impl RuntimeSubagentDispatch {
    /// Construct a default [`RuntimeSubagentDispatch`] backed by a
    /// freshly-built [`FleetSpawner`] over `store` + `scheduler` +
    /// `child_runner`.
    ///
    /// The default construction wires:
    /// - the bundled [`SubagentRegistry`] (`SubagentRegistry::bundled`),
    /// - a freshly-allocated [`ParentLeaseRegistry`],
    /// - a freshly-allocated tracking-only [`QuotaPool`],
    /// - a freshly-allocated [`FleetRegistry`],
    /// - the [`RuntimeChildRunner`] adapter wrapping the
    ///   `scheduler` + `store` pair.
    #[must_use]
    pub fn new(store: Arc<dyn Store>, scheduler: Arc<Scheduler>) -> Self {
        let registry = SubagentRegistry::bundled();
        Self::with_components(
            store.clone(),
            scheduler.clone(),
            registry.clone(),
            Arc::new(FleetRegistry::new()),
            Arc::new(QuotaPool::new()),
            Arc::new(ParentLeaseRegistry::new()),
            Arc::new(RuntimeChildRunner::new(store, scheduler, registry)),
        )
    }

    /// Explicit constructor used by callers that already have a
    /// shared [`FleetRegistry`] / [`QuotaPool`] / [`ParentLeaseRegistry`]
    /// (e.g. tests or the eventual fleet daemon composition root in
    /// Phase 7b).
    #[must_use]
    pub fn with_components(
        store: Arc<dyn Store>,
        _scheduler: Arc<Scheduler>,
        registry: SubagentRegistry,
        fleet_registry: Arc<FleetRegistry>,
        quota: Arc<QuotaPool>,
        leases: Arc<ParentLeaseRegistry>,
        child_runner: Arc<dyn ChildRunner>,
    ) -> Self {
        let spawner = Arc::new(FleetSpawner::with_default_derivation(
            store,
            fleet_registry,
            quota,
            leases,
            child_runner,
            FleetSpawnerConfig::default(),
        ));
        Self { registry, spawner }
    }

    /// Override the bundled subagent registry. Used in tests where a
    /// custom [`SubagentKindSpec`] (e.g. zero-timeout) needs to be
    /// available in addition to the bundled defaults.
    ///
    /// Note: in Phase 7a the [`RuntimeChildRunner`] keeps its own
    /// copy of the registry so it can look up the kind during the
    /// child loop; tests that replace the dispatcher's registry
    /// should construct the dispatcher via [`Self::with_components`]
    /// passing the same registry into a freshly-built
    /// [`RuntimeChildRunner`] for full coverage. The convenience
    /// constructor below preserves the legacy test ergonomics for
    /// the common case where the dispatcher and runner share the
    /// same registry instance.
    #[must_use]
    pub fn with_registry(mut self, registry: SubagentRegistry) -> Self {
        self.registry = registry;
        self
    }
}

#[async_trait]
impl SubagentDispatchHook for RuntimeSubagentDispatch {
    async fn dispatch(&self, request: SubagentDispatchRequest) -> Result<SubagentResult, String> {
        // Resolve the bundled kind early so we can reject unknown
        // kinds without engaging the fleet layer. Mirrors the
        // pre-Phase-7a fast path.
        let Some(kind) = self.registry.get(&request.subagent_type).cloned() else {
            return Ok(SubagentResult::rejected(format!(
                "unknown subagent type '{}'",
                request.subagent_type
            )));
        };

        let parent_ctx = parent_context_from_request(&request);
        let overrides = overrides_from_request(&request, &kind);
        let task_compat = TaskCompatContext {
            subagent_type: request.subagent_type.clone(),
            system_prompt_addendum: request.system_prompt_addendum.clone(),
            parent_permissions: request.parent_permissions.clone(),
            parent_tool_permissions: request.parent_tool_permissions.clone(),
            user_tool_defaults: request.user_tool_defaults.clone(),
            model_override: request.model_override.clone(),
            parent_agent_id: request.parent_agent_id,
            parent_chain: request.parent_chain.clone(),
        };

        let spawn_request = SpawnRequest {
            parent: parent_ctx,
            overrides,
            prompt: request.prompt.clone(),
            originating_user_id: request.originating_user_id.clone(),
            task_compat: Some(task_compat),
        };

        match self.spawner.spawn(spawn_request, SpawnMode::Wait).await {
            Ok(SpawnHandle::Completed(result)) => Ok(result),
            Err(err) => Ok(spawn_error_to_subagent_result(err)),
        }
    }
}

/// Build the synthetic [`ParentContext`] consumed by
/// [`aura_agent_subagent::DefaultDerivation`].
///
/// The legacy task path never carried the parent's
/// [`AgentMode`] / [`ModeProfile`] / `model_id` snapshot — those
/// fields live in the session state today and are not threaded
/// through to [`SubagentDispatchRequest`]. For Phase 7a we
/// pessimistically reconstruct the snapshot from what the request
/// DOES carry and synthesise the rest with the spawn-compatible
/// defaults the deleted in-runtime path implicitly assumed:
///
/// - `mode = AgentMode::Agent` — the only mode that today permits
///   spawning ([`AgentMode::allows_spawn`]).
/// - `mode_profile = ModeProfile { agent: Agent, kernel: Audited,
///   sandbox: Workspace, replay: Live }` — a parent that reached
///   the task tool has by definition cleared workspace-sandbox
///   write permission and the audited kernel mode.
/// - `model_id = ""` — a placeholder; the runner reads the real
///   model id from the kind / request override path, not from the
///   parent context.
/// - `lineage` — reconstructed from `request.parent_chain` so the
///   derivation sees the correct depth.
///
/// Phase 7b removes the placeholder fields by threading the
/// parent's true session snapshot through [`SubagentDispatchRequest`]
/// (an additive request-shape extension).
fn parent_context_from_request(request: &SubagentDispatchRequest) -> ParentContext {
    let lineage = if request.parent_chain.is_empty() {
        SubagentLineage::from_root(request.parent_agent_id)
    } else {
        SubagentLineage {
            root_agent_id: request
                .parent_chain
                .last()
                .copied()
                .unwrap_or(request.parent_agent_id),
            chain: request.parent_chain.clone(),
        }
    };
    let permissions = legacy_permissions_to_modes(&request.parent_permissions);
    let depth = u32::try_from(request.parent_chain.len()).unwrap_or(u32::MAX);
    ParentContext {
        agent_id: request.parent_agent_id,
        depth,
        mode: AgentMode::Agent,
        mode_profile: ModeProfile {
            agent: AgentMode::Agent,
            kernel: KernelMode::Audited,
            sandbox: SandboxMode::Standard,
            replay: ReplayMode::Live,
        },
        permissions,
        model_id: String::new(),
        lineage,
    }
}

/// Build [`SubagentOverrides`] from the parent's request +
/// the resolved kind. Phase 7a-only fields like `subagent_type`,
/// `system_prompt_addendum`, and `user_tool_defaults` are carried
/// out-of-band through [`TaskCompatContext`].
fn overrides_from_request(
    request: &SubagentDispatchRequest,
    kind: &SubagentKindSpec,
) -> SubagentOverrides {
    let narrowed_parent = narrow_permissions(&request.parent_permissions, kind);
    SubagentOverrides {
        mode: None,
        permissions: Some(legacy_permissions_to_modes(&narrowed_parent)),
        kernel_mode: None,
        model_id: request
            .model_override
            .clone()
            .or_else(|| kind.default_model.clone()),
        kind: Some(kind.name.clone()),
        spawn_mode: None,
        join_policy: None,
        replay_mode: None,
        budget: None,
        tool_subset: Some(kind.allowed_tools.clone()),
        isolation_id: None,
    }
}

/// Translate a legacy [`AgentPermissions`] into the layered
/// [`Permissions`] type the fleet layer consumes.
///
/// The translation is intentionally narrow: it copies the held
/// capabilities verbatim. `AgentScope` is dropped because the
/// fleet layer does not model scope today; the legacy executor
/// resolver still honours it via the explicit
/// [`AgentToolPermissions`] path threaded through
/// [`TaskCompatContext`].
fn legacy_permissions_to_modes(legacy: &AgentPermissions) -> Permissions {
    // `AgentPermissions` (re-exported via `aura_core` from
    // `aura_core_permissions`) and `Permissions` share the same
    // `scope` + `capabilities` shape. The clone is a structural
    // copy; the conversion exists purely so the runtime adapter
    // can call into the new `aura-agent-subagent` API surface
    // without leaking layer-permissions types upward.
    Permissions {
        scope: legacy.scope.clone(),
        capabilities: legacy.capabilities.clone(),
    }
}

/// Translate a [`SpawnError`] into the byte-identical
/// [`SubagentResult::Rejected`] shape the pre-Phase-7a runtime
/// produced. Mode/derivation/audit/registry failures all surface as
/// `Rejected { reason }` exit; child-runner failures bubble through
/// the runner's own translation.
fn spawn_error_to_subagent_result(err: SpawnError) -> SubagentResult {
    SubagentResult::rejected(format!("spawn: {err}"))
}

/// [`ChildRunner`] implementation backed by the [`Scheduler`] +
/// [`Store`] pair.
///
/// Owns the byte-identical legacy translation from agent-loop
/// outcome → [`SubagentResult`] so the task tool's wire shape is
/// stable across the Phase 7a refactor.
pub struct RuntimeChildRunner {
    store: Arc<dyn Store>,
    scheduler: Arc<Scheduler>,
    registry: SubagentRegistry,
    spawn_hook: aura_kernel::KernelSpawnHook,
}

impl std::fmt::Debug for RuntimeChildRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeChildRunner")
            .field("registry_kinds", &self.registry.all().len())
            .finish()
    }
}

impl RuntimeChildRunner {
    /// Construct a [`RuntimeChildRunner`] over the supplied store /
    /// scheduler / registry. Same lifetime guarantees as
    /// [`RuntimeSubagentDispatch`]: cheap to clone, internally
    /// shared.
    #[must_use]
    pub fn new(
        store: Arc<dyn Store>,
        scheduler: Arc<Scheduler>,
        registry: SubagentRegistry,
    ) -> Self {
        Self {
            spawn_hook: aura_kernel::KernelSpawnHook::new(store.clone()),
            store,
            scheduler,
            registry,
        }
    }
}

#[async_trait]
impl ChildRunner for RuntimeChildRunner {
    async fn run(&self, ctx: ChildRunContext) -> Result<SubagentResult, ChildRunError> {
        let originating_user_id = ctx.originating_user_id.clone();
        let compat = ctx.task_compat.ok_or_else(|| {
            ChildRunError::Internal(
                "RuntimeChildRunner requires the Phase 7a TaskCompatContext".to_string(),
            )
        })?;

        let Some(kind) = self.registry.get(&compat.subagent_type).cloned() else {
            return Ok(SubagentResult::rejected(format!(
                "unknown subagent type '{}'",
                compat.subagent_type
            )));
        };

        let child_permissions = narrow_permissions(&compat.parent_permissions, &kind);
        let child_tool_permissions = narrowed_tool_permissions(&compat, &kind);
        let child_spec = aura_kernel::ChildAgentSpec {
            name: kind.name.clone(),
            role: format!("subagent:{}", kind.name),
            permissions: child_permissions.clone(),
            tool_permissions: Some(child_tool_permissions.clone()),
            parent_tool_permissions: compat.parent_tool_permissions.clone(),
            system_prompt_override: Some(system_prompt_for(&kind, &compat)),
            preassigned_agent_id: None,
        };

        let spawn_outcome = {
            use aura_kernel::SpawnHook;
            self.spawn_hook
                .spawn_child(
                    &compat.parent_agent_id,
                    originating_user_id.as_deref(),
                    child_spec,
                )
                .await
                .map_err(|e| ChildRunError::Internal(format!("spawn child: {e}")))?
        };
        let child_agent_id = spawn_outcome.child_agent_id;

        let tx = Transaction::new_chained(
            child_agent_id,
            TransactionType::UserPrompt,
            Bytes::from(ctx.prompt.clone().into_bytes()),
            None,
        );
        self.store
            .enqueue_tx(&tx)
            .map_err(|e| ChildRunError::Internal(format!("enqueue child prompt: {e}")))?;

        let child_model = compat
            .model_override
            .as_deref()
            .or(kind.default_model.as_deref())
            .unwrap_or(aura_reasoner::ENV_FALLBACK_MODEL)
            .to_string();
        if let Some(parent) = self
            .scheduler
            .identity_registry()
            .get(compat.parent_agent_id)
        {
            let mut child_identity = parent.clone();
            child_identity.model = child_model.clone();
            child_identity.system_prompt = system_prompt_for(&kind, &compat);
            self.scheduler
                .identity_registry()
                .register(child_agent_id, child_identity);
        }
        let loop_config = loop_config_for(&kind, &child_model);
        let policy = policy_for(child_permissions, child_tool_permissions, &compat);
        if kind.budget.timeout_ms == 0 {
            return Ok(SubagentResult {
                child_agent_id: Some(child_agent_id),
                final_message: String::new(),
                total_input_tokens: 0,
                total_output_tokens: 0,
                files_changed: Vec::new(),
                exit: SubagentExit::Timeout,
            });
        }
        let processed = match tokio::time::timeout(
            Duration::from_millis(kind.budget.timeout_ms),
            self.scheduler.schedule_agent_with_overrides(
                child_agent_id,
                Some(loop_config),
                Some(policy),
            ),
        )
        .await
        {
            Ok(result) => result.map_err(|e| ChildRunError::Internal(format!("schedule: {e}")))?,
            Err(_) => {
                return Ok(SubagentResult {
                    child_agent_id: Some(child_agent_id),
                    final_message: String::new(),
                    total_input_tokens: 0,
                    total_output_tokens: 0,
                    files_changed: Vec::new(),
                    exit: SubagentExit::Timeout,
                });
            }
        };

        let Some(result) = processed.last_result else {
            warn!(
                child_agent_id = %child_agent_id,
                "fleet child runner: agent loop produced no result"
            );
            return Ok(SubagentResult {
                child_agent_id: Some(child_agent_id),
                final_message: String::new(),
                total_input_tokens: 0,
                total_output_tokens: 0,
                files_changed: Vec::new(),
                exit: SubagentExit::Failed {
                    reason: "child processed no agent loop result".into(),
                },
            });
        };

        let exit = result
            .llm_error
            .as_ref()
            .map_or(SubagentExit::Completed, |reason| SubagentExit::Failed {
                reason: reason.clone(),
            });
        Ok(SubagentResult {
            child_agent_id: Some(child_agent_id),
            final_message: result.total_text,
            total_input_tokens: result.total_input_tokens,
            total_output_tokens: result.total_output_tokens,
            files_changed: result
                .file_changes
                .into_iter()
                .map(|change| change.path)
                .collect(),
            exit,
        })
    }
}

fn narrow_permissions(parent: &AgentPermissions, kind: &SubagentKindSpec) -> AgentPermissions {
    let capabilities = parent
        .capabilities
        .iter()
        .filter(|held| {
            kind.allowed_capabilities
                .iter()
                .any(|allowed| held.satisfies(allowed))
        })
        .cloned()
        .collect();
    AgentPermissions {
        scope: parent.scope.clone(),
        capabilities,
    }
}

fn narrowed_tool_permissions(
    compat: &TaskCompatContext,
    kind: &SubagentKindSpec,
) -> AgentToolPermissions {
    let mut per_tool = BTreeMap::new();
    for tool in &kind.allowed_tools {
        let parent_state = resolve_effective_permission(
            &compat.user_tool_defaults,
            compat.parent_tool_permissions.as_ref(),
            tool,
        );
        per_tool.insert(tool.clone(), parent_state);
    }
    AgentToolPermissions { per_tool }
}

fn policy_for(
    permissions: AgentPermissions,
    tool_permissions: AgentToolPermissions,
    compat: &TaskCompatContext,
) -> PolicyConfig {
    let fallback = match compat.user_tool_defaults.mode {
        UserDefaultMode::AutoReview => ToolState::Ask,
        _ => ToolState::Deny,
    };
    let user_default = UserToolDefaults::default_permissions(BTreeMap::new(), fallback);
    PolicyConfig::default()
        .with_agent_permissions(permissions)
        .with_user_default(user_default)
        .with_agent_override(Some(tool_permissions))
}

fn loop_config_for(kind: &SubagentKindSpec, model: &str) -> AgentLoopConfig {
    let mut config = AgentLoopConfig {
        system_prompt: kind.system_prompt.clone(),
        max_iterations: kind.budget.max_iterations as usize,
        ..AgentLoopConfig::for_agent(model)
    };
    if let Some(max_tokens) = kind.budget.max_tokens {
        config.max_tokens = max_tokens;
    }
    config
}

fn system_prompt_for(kind: &SubagentKindSpec, compat: &TaskCompatContext) -> String {
    match compat.system_prompt_addendum.as_deref() {
        Some(addendum) if !addendum.trim().is_empty() => {
            format!("{}\n\n{}", kind.system_prompt, addendum)
        }
        _ => kind.system_prompt.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::Scheduler;
    use aura_core::{AgentId, AgentScope, Capability};
    use aura_reasoner::MockProvider;
    use aura_store::{ReadStore, RocksStore};
    use aura_tools::ToolCatalog;

    fn compat_for(
        parent_agent_id: AgentId,
        parent_permissions: AgentPermissions,
    ) -> TaskCompatContext {
        TaskCompatContext {
            subagent_type: "explore".into(),
            system_prompt_addendum: None,
            parent_permissions,
            parent_tool_permissions: None,
            user_tool_defaults: UserToolDefaults::full_access(),
            model_override: None,
            parent_agent_id,
            parent_chain: Vec::new(),
        }
    }

    #[test]
    fn narrow_permissions_keeps_only_kind_allowed_caps() {
        let parent = AgentPermissions {
            scope: AgentScope::default(),
            capabilities: vec![Capability::SpawnAgent, Capability::ReadAgent],
        };
        let mut kind = SubagentRegistry::bundled()
            .get("general_purpose")
            .unwrap()
            .clone();
        kind.allowed_capabilities = vec![Capability::ReadAgent];
        let narrowed = narrow_permissions(&parent, &kind);
        assert_eq!(narrowed.capabilities, vec![Capability::ReadAgent]);
    }

    #[test]
    fn policy_fallback_denies_tools_outside_allowlist() {
        let compat = compat_for(
            AgentId::generate(),
            AgentPermissions {
                scope: AgentScope::default(),
                capabilities: vec![Capability::SpawnAgent],
            },
        );
        let registry = SubagentRegistry::bundled();
        let kind = registry.get("explore").unwrap();
        let tool_permissions = narrowed_tool_permissions(&compat, kind);
        let policy = policy_for(AgentPermissions::empty(), tool_permissions, &compat);
        assert_eq!(
            resolve_effective_permission(
                &policy.user_default,
                policy.agent_override.as_ref(),
                "write_file",
            ),
            ToolState::Deny
        );
        assert_eq!(
            resolve_effective_permission(
                &policy.user_default,
                policy.agent_override.as_ref(),
                "read_file",
            ),
            ToolState::Allow
        );
    }

    #[tokio::test]
    async fn dispatch_runs_child_and_records_parent_and_child_logs() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let store = Arc::new(RocksStore::open(dir.path().join("db"), false).unwrap());
        let provider = Arc::new(MockProvider::simple_response("child done"));
        let catalog = ToolCatalog::default();
        let scheduler = Arc::new(Scheduler::new(
            store.clone(),
            provider,
            Vec::new(),
            catalog.executor_builtin_tools(),
            workspace.path().to_path_buf(),
            None,
        ));
        let dispatch = RuntimeSubagentDispatch::new(store.clone(), scheduler);
        let parent_agent_id = AgentId::generate();

        let result = dispatch
            .dispatch(SubagentDispatchRequest {
                parent_agent_id,
                subagent_type: "explore".into(),
                prompt: "summarize".into(),
                originating_user_id: Some("user".into()),
                parent_chain: Vec::new(),
                model_override: None,
                system_prompt_addendum: None,
                parent_permissions: AgentPermissions {
                    scope: AgentScope::default(),
                    capabilities: vec![Capability::SpawnAgent],
                },
                parent_tool_permissions: None,
                user_tool_defaults: UserToolDefaults::full_access(),
            })
            .await
            .unwrap();

        assert!(matches!(result.exit, SubagentExit::Completed));
        assert_eq!(result.final_message, "child done");
        let child_id = result.child_agent_id.expect("child id");
        assert!(
            !store
                .scan_record(parent_agent_id, 1, 10)
                .unwrap()
                .is_empty(),
            "spawn should record parent delegation"
        );
        let child_entries = store.scan_record(child_id, 1, 10).unwrap();
        assert!(
            child_entries
                .iter()
                .any(|entry| entry.tx.tx_type == TransactionType::AgentMsg),
            "child should record final assistant message"
        );
    }

    #[tokio::test]
    async fn dispatch_returns_timeout_for_exhausted_budget() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let store = Arc::new(RocksStore::open(dir.path().join("db"), false).unwrap());
        let provider = Arc::new(MockProvider::simple_response("late"));
        let catalog = ToolCatalog::default();
        let scheduler = Arc::new(Scheduler::new(
            store.clone(),
            provider,
            Vec::new(),
            catalog.executor_builtin_tools(),
            workspace.path().to_path_buf(),
            None,
        ));
        let registry = SubagentRegistry::bundled();
        let mut kind = registry.get("explore").unwrap().clone();
        kind.name = "instant_timeout".into();
        kind.budget.timeout_ms = 0;
        let registry = SubagentRegistry::from_specs(vec![kind]);
        let store_for_runner = store.clone();
        let scheduler_for_runner = scheduler.clone();
        let runner = Arc::new(RuntimeChildRunner::new(
            store_for_runner,
            scheduler_for_runner,
            registry.clone(),
        ));
        let dispatch = RuntimeSubagentDispatch::with_components(
            store,
            scheduler,
            registry,
            Arc::new(FleetRegistry::new()),
            Arc::new(QuotaPool::new()),
            Arc::new(ParentLeaseRegistry::new()),
            runner,
        );

        let result = dispatch
            .dispatch(SubagentDispatchRequest {
                parent_agent_id: AgentId::generate(),
                subagent_type: "instant_timeout".into(),
                prompt: "summarize".into(),
                originating_user_id: Some("user".into()),
                parent_chain: Vec::new(),
                model_override: None,
                system_prompt_addendum: None,
                parent_permissions: AgentPermissions {
                    scope: AgentScope::default(),
                    capabilities: vec![Capability::SpawnAgent],
                },
                parent_tool_permissions: None,
                user_tool_defaults: UserToolDefaults::full_access(),
            })
            .await
            .unwrap();

        assert!(matches!(result.exit, SubagentExit::Timeout));
    }
}
