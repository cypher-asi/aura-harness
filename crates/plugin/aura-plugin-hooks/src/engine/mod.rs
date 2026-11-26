//! Hook engine: registration + firing.
//!
//! Phase 4c shipped the engine shell and the manual `fire` surface
//! used by the integration test. Phase 8 fires hooks for real at
//! every lifecycle point in the agent loop / agent host.
//!
//! ## Invariants ([rules.md §13])
//!
//! - Hook command resolution mirrors the Codex / Claude contract:
//!   - a bare filename (no `/` or `\`) is passed verbatim so the OS
//!     PATH lookup applies — do NOT join with `plugin_root`.
//!   - a relative path WITH a separator joins to `plugin_root`.
//!   - an absolute path is used as-is.
//! - **Empty engine** firings are O(1): [`HookEngine::is_empty`]
//!   answers `true` immediately when no hooks are registered for an
//!   event. Phase 8's backward-compat invariant requires
//!   [`HookEngine::fire_event`] to short-circuit before allocating a
//!   ctx for empty engines.
//! - **Per-handler 5-second timeout**: every hook spawn is wrapped
//!   in a wall-clock timeout. Overruns produce
//!   [`crate::HookOutcome::TimedOut`] and the engine continues with
//!   the remaining handlers.
//! - **First-terminal-wins**: the loop short-circuits on the first
//!   `Block` / `Approve` / `Deny`. `Replace` updates the carried
//!   payload and the loop continues; the LAST `Replace` wins.
//! - Spawned hooks see an explicitly scrubbed env (see
//!   [`crate::sandbox::scrubbed_env`]) plus the canonical Aura +
//!   Codex/Claude aliases.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use crate::context::HookFiringContext;
use crate::ctx::{CtxMeta, HookCtx};
use crate::error::HookError;
use crate::event::HookEvent;
use crate::outcome::{AggregateOutcome, HookOutcome};
use crate::sandbox::InjectedEnv;

mod process;

/// Per-hook wall-clock timeout. Overruns produce
/// [`HookOutcome::TimedOut`] and the firing loop continues. 5s is
/// the documented Phase 8 default.
pub const DEFAULT_HOOK_TIMEOUT: Duration = Duration::from_secs(5);

/// Per-hook stdout capture cap. 256 KiB matches the documented
/// budget. Larger output is truncated with a `tracing::warn!`.
pub const HOOK_STDOUT_CAP_BYTES: usize = 256 * 1024;

/// A single hook registered against an event. Owned by the engine.
#[derive(Clone, Debug)]
pub struct RegisteredHook {
    /// Plugin id that contributed this hook. Used for diagnostics
    /// and to scope `Drop`-time cleanup decisions.
    pub plugin_id: String,
    /// Event this hook listens to.
    pub event: HookEvent,
    /// Command to spawn. See the module-level docs for the path
    /// resolution rules.
    pub command: String,
    /// Command-line arguments. Default empty.
    pub args: Vec<String>,
    /// Cache version directory for the plugin. Used both to inject
    /// `PLUGIN_ROOT` and to resolve relative `command` paths.
    pub plugin_root: PathBuf,
    /// Plugin-author allowlisted env overrides. Merged into the
    /// spawned process's env after the canonical
    /// [`crate::sandbox::scrubbed_env`] payload.
    pub env: BTreeMap<String, String>,
}

/// Summary of a single Phase 4c [`HookEngine::fire`] call. Phase 8
/// callers use [`AggregateOutcome`] instead.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookFireSummary {
    /// Number of hooks that returned an exit code of 0.
    pub succeeded: u32,
    /// Number of hooks that failed (spawn error or non-zero exit).
    pub failed: u32,
}

/// Registration + firing surface for lifecycle hooks. Construct via
/// [`HookEngine::new`]; clone the resulting `Arc` for cheap sharing
/// across runtime tasks.
#[derive(Default, Debug)]
pub struct HookEngine {
    by_event: Mutex<HashMap<HookEvent, Vec<RegisteredHook>>>,
    /// Per-handler wall-clock timeout. Defaults to
    /// [`DEFAULT_HOOK_TIMEOUT`]; tests may override via
    /// [`HookEngine::with_timeout`].
    timeout: Duration,
}

impl HookEngine {
    /// Construct a new, empty engine.
    #[must_use]
    pub fn new() -> Self {
        Self {
            by_event: Mutex::new(HashMap::new()),
            timeout: DEFAULT_HOOK_TIMEOUT,
        }
    }

    /// Override the per-handler wall-clock timeout (test hook —
    /// the default is [`DEFAULT_HOOK_TIMEOUT`]).
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Register a hook. Hooks fire in registration order for the
    /// same event.
    pub fn register(&self, hook: RegisteredHook) {
        let mut guard = self
            .by_event
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.entry(hook.event).or_default().push(hook);
    }

    /// **Phase 8 short-circuit gate**. `true` when no hooks are
    /// registered for `event`. The empty-install backward-compat
    /// invariant requires every lifecycle firing site to gate on
    /// this method before allocating a ctx.
    #[must_use]
    pub fn is_empty(&self, event: HookEvent) -> bool {
        let guard = self
            .by_event
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.get(&event).is_none_or(Vec::is_empty)
    }

    /// Count of registered hooks for an event.
    #[must_use]
    pub fn registered_count(&self, event: HookEvent) -> usize {
        let guard = self
            .by_event
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.get(&event).map_or(0, Vec::len)
    }

    /// Phase 4c manual fire surface. Retained for the
    /// `hook_event_smoke` integration test; Phase 8 callers use
    /// [`Self::fire_event`] instead.
    ///
    /// # Errors
    ///
    /// Reserved for future engine-level invariant violations. Phase
    /// 4c always returns `Ok`.
    pub fn fire(&self, ctx: &HookFiringContext) -> Result<HookFireSummary, HookError> {
        let mut summary = HookFireSummary::default();
        let registered = {
            let guard = self
                .by_event
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.get(&ctx.event).cloned().unwrap_or_default()
        };
        if registered.is_empty() {
            return Ok(summary);
        }
        for hook in &registered {
            match self.spawn_phase4c(hook, ctx) {
                Ok(()) => summary.succeeded += 1,
                Err(err) => {
                    tracing::warn!(
                        plugin_id = %hook.plugin_id,
                        event = ?hook.event,
                        error = %err,
                        "hook subprocess failed; continuing with remaining hooks"
                    );
                    summary.failed += 1;
                }
            }
        }
        Ok(summary)
    }

    /// **Phase 8** firing entry point.
    ///
    /// Builds the canonical [`InjectedEnv`] for `ctx`, fires every
    /// registered handler in order, and aggregates their outcomes
    /// per the rules documented on [`AggregateOutcome`]:
    ///
    /// 1. First terminal outcome (`Block` / `Approve` / `Deny`) wins
    ///    and short-circuits subsequent handlers.
    /// 2. Otherwise, the LAST `Replace` wins.
    /// 3. Otherwise, `Continue`.
    ///
    /// Empty-engine firings short-circuit immediately at the
    /// `is_empty` gate so an empty install carries zero overhead
    /// beyond a single `HashMap` lookup.
    pub fn fire_event<C>(&self, ctx: &C, aura_home: &Path) -> AggregateOutcome
    where
        C: HookCtx,
    {
        let event = ctx.event();
        if self.is_empty(event) {
            return AggregateOutcome::empty();
        }
        let registered = {
            let guard = self
                .by_event
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.get(&event).cloned().unwrap_or_default()
        };
        if registered.is_empty() {
            return AggregateOutcome::empty();
        }

        let mut agg = AggregateOutcome::empty();
        let event_name = event.as_str();
        let extra_from_ctx = ctx.extra_env();

        for hook in &registered {
            let injected = InjectedEnv {
                event_name,
                aura_home: aura_home.to_path_buf(),
                plugin_root: hook.plugin_root.clone(),
                session_id: ctx.meta().session_id.clone(),
                agent_id: ctx.meta().agent_id.clone(),
                parent_agent_id: ctx.meta().parent_agent_id.clone(),
                extra: merge_extra(&extra_from_ctx, &hook.env),
            };
            let outcome = self.spawn_phase8(hook, &injected);
            agg.ran += 1;
            if matches!(outcome, HookOutcome::TimedOut) {
                agg.timed_out += 1;
            }
            match &outcome {
                HookOutcome::Continue | HookOutcome::TimedOut => {
                    // Default: keep going.
                }
                HookOutcome::Replace { .. } => {
                    // Latest replace wins; keep going so subsequent
                    // handlers can refine.
                    agg.decision = outcome;
                }
                HookOutcome::Block { .. } | HookOutcome::Approve | HookOutcome::Deny { .. } => {
                    agg.decision = outcome;
                    break;
                }
            }
        }
        agg
    }
}

/// Cheap-cloneable shared engine handle for runtime crates.
pub type SharedHookEngine = Arc<HookEngine>;

fn merge_extra(
    from_ctx: &BTreeMap<String, String>,
    from_hook: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut out = from_ctx.clone();
    for (k, v) in from_hook {
        out.insert(k.clone(), v.clone());
    }
    out
}

/// Build a [`CtxMeta`] from string fields. Convenience constructor
/// the runtime call sites use.
#[must_use]
pub fn make_meta(
    session_id: impl Into<String>,
    agent_id: impl Into<String>,
    parent_agent_id: Option<String>,
) -> CtxMeta {
    CtxMeta {
        session_id: session_id.into(),
        agent_id: agent_id.into(),
        parent_agent_id,
    }
}

#[cfg(test)]
mod tests;
