//! Idempotent / bridge-aware wrapper around
//! [`DomainApi::transition_task`](aura_tools::domain_tools::DomainApi::transition_task).
//!
//! The storage service enforces a strict task state machine and rejects
//! both no-op transitions (e.g. `done → done`) and certain direct jumps
//! (e.g. `failed → in_progress`, `ready → failed`). Previously the dev
//! loop called `transition_task` unconditionally and surfaced every
//! rejection as `WARN`:
//!
//! ```text
//! Failed to sync task done status to backend
//!   error=HTTP 400 … Invalid status transition: 'done' → 'done'
//! Failed to transition task to in_progress (continuing anyway)
//!   error=HTTP 400 … Invalid status transition: 'failed' → 'in_progress'
//! ```
//!
//! Those warnings were noise — the client knew enough to either skip
//! the call (idempotent) or route through an intermediate status. This
//! helper encapsulates that knowledge so both `tick.rs` and
//! `task_run.rs` share a single, tested path.
//!
//! # Behaviour
//!
//! 1. Pre-fetches the task's current status via `get_task`.
//! 2. If `current == target`, **skips** the transition entirely and
//!    logs at `debug` (no WARN).
//! 3. For known-disallowed direct transitions, bridges via an
//!    intermediate status (failures of the bridge step are logged at
//!    `debug` but do not abort the final transition — the server is
//!    the source of truth on rejections).
//! 4. Falls back to a direct `transition_task` on `get_task` failure so
//!    the helper never degrades the at-least-once semantics the
//!    previous callers relied on.

use aura_tools::domain_tools::DomainApi;
use tracing::debug;

/// Safely transition a task to `target`, handling idempotent no-ops and
/// bridging known-disallowed direct transitions. See module docs for the
/// exact rules.
///
/// Returns `Ok(())` on success, idempotent skip, or successful bridge.
/// Returns `Err(_)` only if the *final* transition step fails — caller
/// is expected to log that at `warn` since it indicates real backend
/// divergence (not a client state-machine bug).
pub(crate) async fn safe_transition(
    domain: &dyn DomainApi,
    task_id: &str,
    target: &str,
) -> anyhow::Result<()> {
    let current = match domain.get_task(task_id, None).await {
        Ok(t) => t.status,
        Err(e) => {
            // Fall back to the previous behaviour: if we can't read
            // the task, attempt the transition directly so we don't
            // silently drop a state change. The direct call will
            // surface any genuine rejection to the caller.
            debug!(
                task_id,
                target,
                error = %e,
                "safe_transition: get_task lookup failed; attempting direct transition"
            );
            return domain
                .transition_task(task_id, target, None)
                .await
                .map(|_| ());
        }
    };

    if current == target {
        debug!(
            task_id,
            status = %target,
            "safe_transition: already in target state; skipping no-op transition"
        );
        return Ok(());
    }

    if let Some(intermediate) = bridge_for(&current, target) {
        if let Err(e) = domain
            .transition_task(task_id, intermediate, None)
            .await
        {
            // A failed bridge step is not fatal — the server may have
            // moved underneath us, or the bridge may have been made
            // unnecessary by an earlier call. Log and let the final
            // transition attempt speak for itself.
            debug!(
                task_id,
                from = %current,
                via = %intermediate,
                to = %target,
                error = %e,
                "safe_transition: bridge step failed; continuing to final transition"
            );
        }
    }

    domain
        .transition_task(task_id, target, None)
        .await
        .map(|_| ())
}

/// Map of known-disallowed direct transitions to the intermediate
/// status that must be visited first. Extend this table when storage
/// surfaces new `Invalid status transition` rejections in the logs.
fn bridge_for(current: &str, target: &str) -> Option<&'static str> {
    match (current, target) {
        // Start a freshly-queued task: pending → ready → in_progress.
        // This mirrors the original `transition_to_in_progress`
        // behaviour where a `pending` task was first promoted to
        // `ready` before being moved to `in_progress`.
        ("pending", "in_progress") => Some("ready"),
        // Restart a failed task: failed → ready → in_progress. The
        // direct `failed → in_progress` jump is rejected by storage
        // (observed: `HTTP 400 … 'failed' → 'in_progress'`).
        ("failed", "in_progress") => Some("ready"),
        // `ready → failed` is rejected by storage; bridge via
        // `in_progress`. Matches the note in
        // `aura-os-server/src/handlers/dev_loop.rs` that the
        // terminal-failure transition must bridge `ready →
        // in_progress → failed`.
        ("ready", "failed") => Some("in_progress"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use aura_core::PermissionLevel;
    use aura_tools::domain_tools::{
        CreateSessionParams, MessageDescriptor, ProjectDescriptor, ProjectUpdate,
        SaveMessageParams, SessionDescriptor, SpecDescriptor, TaskDescriptor, TaskUpdate,
    };
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Minimal `DomainApi` double that records every `transition_task`
    /// call and lets each test drive the status machine via
    /// `set_status` / `reject_transition`. Only the two methods used by
    /// `safe_transition` (`get_task` + `transition_task`) are
    /// meaningful; the rest `unimplemented!()` so any accidental
    /// reliance panics loudly in CI.
    struct FakeDomain {
        status: Mutex<String>,
        // (from, to) pairs that should return `Err` when attempted.
        reject: Mutex<Vec<(String, String)>>,
        // Record of every transition target the helper called, in
        // order. Tests assert against this to verify bridging and
        // no-op skipping.
        calls: Mutex<Vec<String>>,
        fail_get: Mutex<bool>,
    }

    impl FakeDomain {
        fn new(initial: &str) -> Self {
            Self {
                status: Mutex::new(initial.to_string()),
                reject: Mutex::new(Vec::new()),
                calls: Mutex::new(Vec::new()),
                fail_get: Mutex::new(false),
            }
        }
        fn reject_transition(&self, from: &str, to: &str) {
            self.reject
                .lock()
                .unwrap()
                .push((from.to_string(), to.to_string()));
        }
        fn set_get_failure(&self, fail: bool) {
            *self.fail_get.lock().unwrap() = fail;
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl DomainApi for FakeDomain {
        async fn list_specs(
            &self,
            _: &str,
            _: Option<&str>,
        ) -> anyhow::Result<Vec<SpecDescriptor>> {
            unimplemented!()
        }
        async fn get_spec(&self, _: &str, _: Option<&str>) -> anyhow::Result<SpecDescriptor> {
            unimplemented!()
        }
        async fn create_spec(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: u32,
            _: Option<&str>,
        ) -> anyhow::Result<SpecDescriptor> {
            unimplemented!()
        }
        async fn update_spec(
            &self,
            _: &str,
            _: Option<&str>,
            _: Option<&str>,
            _: Option<&str>,
        ) -> anyhow::Result<SpecDescriptor> {
            unimplemented!()
        }
        async fn delete_spec(&self, _: &str, _: Option<&str>) -> anyhow::Result<()> {
            unimplemented!()
        }
        async fn list_tasks(
            &self,
            _: &str,
            _: Option<&str>,
            _: Option<&str>,
        ) -> anyhow::Result<Vec<TaskDescriptor>> {
            unimplemented!()
        }
        async fn create_task(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &str,
            _: &[String],
            _: u32,
            _: Option<&str>,
        ) -> anyhow::Result<TaskDescriptor> {
            unimplemented!()
        }
        async fn update_task(
            &self,
            _: &str,
            _: TaskUpdate,
            _: Option<&str>,
        ) -> anyhow::Result<TaskDescriptor> {
            unimplemented!()
        }
        async fn delete_task(&self, _: &str, _: Option<&str>) -> anyhow::Result<()> {
            unimplemented!()
        }
        async fn transition_task(
            &self,
            task_id: &str,
            status: &str,
            _: Option<&str>,
        ) -> anyhow::Result<TaskDescriptor> {
            self.calls.lock().unwrap().push(status.to_string());
            let current = self.status.lock().unwrap().clone();
            if self
                .reject
                .lock()
                .unwrap()
                .iter()
                .any(|(f, t)| f == &current && t == status)
            {
                return Err(anyhow::anyhow!(
                    "HTTP 400 Bad Request: Invalid status transition: '{}' → '{}'",
                    current,
                    status
                ));
            }
            *self.status.lock().unwrap() = status.to_string();
            Ok(TaskDescriptor {
                id: task_id.to_string(),
                spec_id: String::new(),
                project_id: String::new(),
                title: String::new(),
                description: String::new(),
                status: status.to_string(),
                dependencies: Vec::new(),
                order: 0,
            })
        }
        async fn claim_next_task(
            &self,
            _: &str,
            _: &str,
            _: Option<&str>,
        ) -> anyhow::Result<Option<TaskDescriptor>> {
            unimplemented!()
        }
        async fn get_task(
            &self,
            task_id: &str,
            _: Option<&str>,
        ) -> anyhow::Result<TaskDescriptor> {
            if *self.fail_get.lock().unwrap() {
                return Err(anyhow::anyhow!("simulated get_task failure"));
            }
            Ok(TaskDescriptor {
                id: task_id.to_string(),
                spec_id: String::new(),
                project_id: String::new(),
                title: String::new(),
                description: String::new(),
                status: self.status.lock().unwrap().clone(),
                dependencies: Vec::new(),
                order: 0,
            })
        }
        async fn get_project(
            &self,
            _: &str,
            _: Option<&str>,
        ) -> anyhow::Result<ProjectDescriptor> {
            unimplemented!()
        }
        async fn update_project(
            &self,
            _: &str,
            _: ProjectUpdate,
            _: Option<&str>,
        ) -> anyhow::Result<ProjectDescriptor> {
            unimplemented!()
        }
        async fn create_log(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: Option<&str>,
            _: Option<&serde_json::Value>,
            _: Option<&str>,
        ) -> anyhow::Result<serde_json::Value> {
            unimplemented!()
        }
        async fn list_logs(
            &self,
            _: &str,
            _: Option<&str>,
            _: Option<u64>,
            _: Option<&str>,
        ) -> anyhow::Result<serde_json::Value> {
            unimplemented!()
        }
        async fn get_project_stats(
            &self,
            _: &str,
            _: Option<&str>,
        ) -> anyhow::Result<serde_json::Value> {
            unimplemented!()
        }
        async fn list_messages(
            &self,
            _: &str,
            _: &str,
        ) -> anyhow::Result<Vec<MessageDescriptor>> {
            unimplemented!()
        }
        async fn save_message(&self, _: SaveMessageParams) -> anyhow::Result<()> {
            unimplemented!()
        }
        async fn create_session(
            &self,
            _: CreateSessionParams,
        ) -> anyhow::Result<SessionDescriptor> {
            unimplemented!()
        }
        async fn get_active_session(
            &self,
            _: &str,
        ) -> anyhow::Result<Option<SessionDescriptor>> {
            unimplemented!()
        }
        async fn orbit_api_call(
            &self,
            _: &str,
            _: &str,
            _: Option<&serde_json::Value>,
            _: Option<&str>,
        ) -> anyhow::Result<String> {
            unimplemented!()
        }
        async fn network_api_call(
            &self,
            _: &str,
            _: &str,
            _: Option<&serde_json::Value>,
            _: Option<&str>,
        ) -> anyhow::Result<String> {
            unimplemented!()
        }
        async fn get_agent_permissions(
            &self,
            _: &str,
            _: Option<&str>,
        ) -> anyhow::Result<Option<HashMap<String, PermissionLevel>>> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn skips_noop_transition_when_already_in_target_state() {
        // Reproduces the `done → done` WARN in the harness logs: the
        // automaton's success path previously re-posted `done` after
        // the agent's `task_done` tool had already moved the task to
        // `done` server-side. The helper must SKIP the POST entirely.
        let domain = FakeDomain::new("done");
        // Configure the fake to reject the redundant transition so
        // the test fails loudly if the helper ever attempts it.
        domain.reject_transition("done", "done");

        safe_transition(&domain, "t1", "done").await.expect("skip");

        assert!(
            domain.calls().is_empty(),
            "safe_transition should not call transition_task when already in target; got {:?}",
            domain.calls()
        );
    }

    #[tokio::test]
    async fn bridges_pending_to_in_progress_via_ready() {
        // Mirrors the original `transition_to_in_progress` behaviour:
        // a freshly-queued task in `pending` must be promoted through
        // `ready` before being moved to `in_progress`.
        let domain = FakeDomain::new("pending");
        domain.reject_transition("pending", "in_progress");

        safe_transition(&domain, "t-pending", "in_progress")
            .await
            .expect("bridged");

        assert_eq!(
            domain.calls(),
            vec!["ready", "in_progress"],
            "expected pending→ready→in_progress bridge"
        );
    }

    #[tokio::test]
    async fn bridges_failed_to_in_progress_via_ready() {
        // Reproduces the `failed → in_progress` WARN: retrying a
        // previously-failed task must go through `ready` because
        // storage rejects the direct jump.
        let domain = FakeDomain::new("failed");
        domain.reject_transition("failed", "in_progress");

        safe_transition(&domain, "t2", "in_progress")
            .await
            .expect("bridged");

        assert_eq!(
            domain.calls(),
            vec!["ready", "in_progress"],
            "expected failed→ready→in_progress bridge"
        );
    }

    #[tokio::test]
    async fn passes_through_ordinary_transition_unchanged() {
        // Control case: `in_progress → done` is a normal transition
        // and must be called exactly once, with no bridging.
        let domain = FakeDomain::new("in_progress");

        safe_transition(&domain, "t3", "done").await.expect("ok");

        assert_eq!(domain.calls(), vec!["done"]);
    }

    #[tokio::test]
    async fn falls_back_to_direct_transition_on_lookup_failure() {
        // If `get_task` fails we can't know the current status, so
        // the helper must still attempt the transition (keeping the
        // previous at-least-once semantics the callers relied on).
        let domain = FakeDomain::new("in_progress");
        domain.set_get_failure(true);

        safe_transition(&domain, "t4", "done").await.expect("ok");

        assert_eq!(domain.calls(), vec!["done"]);
    }
}
