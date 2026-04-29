//! Idempotent / bridge-aware wrapper around
//! [`DomainApi::transition_task`](aura_tools::domain_tools::DomainApi::transition_task).
//!
//! The storage service is the source of truth for task status. This
//! helper reads the current status, skips proven no-ops, bridges the
//! direct transitions storage rejects, and returns an explicit outcome
//! so callers decide whether to log, continue, or fail.
//!
//! Lookup failures are not guessed around: a non-404 `get_task` error is
//! surfaced as [`TransitionError::LookupFailed`] instead of falling back
//! to a blind direct transition. The one explicit local-only policy is a
//! task lookup 404, which becomes [`TransitionOutcome::LocalOnlyMissing`]
//! because the harness can mint in-process tasks that never reach
//! `aura-storage`.

use aura_tools::domain_tools::DomainApi;
use tracing::debug;

/// Transition result after storage sync was attempted or intentionally skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransitionOutcome {
    /// Storage accepted the target transition. The helper may also have
    /// applied an intermediate bridge first.
    Applied,
    /// The task was already in the target status, so no storage write was issued.
    AlreadyInTarget,
    /// Storage returned task-not-found on lookup. This is the explicit
    /// local-only harness policy; callers should generally log at debug
    /// and continue.
    LocalOnlyMissing,
}

/// Failure modes that should reach the caller instead of being hidden by fallback calls.
#[derive(Debug)]
pub(crate) enum TransitionError {
    LookupFailed {
        task_id: String,
        target: String,
        source: anyhow::Error,
    },
    TransitionFailed {
        task_id: String,
        target: String,
        source: anyhow::Error,
    },
}

impl std::fmt::Display for TransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LookupFailed {
                task_id,
                target,
                source,
            } => write!(
                f,
                "failed to look up task {task_id} before transition to {target}: {source}"
            ),
            Self::TransitionFailed {
                task_id,
                target,
                source,
            } => write!(
                f,
                "failed to transition task {task_id} to {target}: {source}"
            ),
        }
    }
}

impl std::error::Error for TransitionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::LookupFailed { source, .. } | Self::TransitionFailed { source, .. } => {
                Some(source.as_ref())
            }
        }
    }
}

/// Detect a "task not found" / HTTP 404 response from the storage service.
///
/// Removal note: replace this adapter when `DomainApi` exposes typed
/// status-code errors. Until then, keep all status-string matching
/// centralized here so callers can consume [`TransitionOutcome`] instead
/// of parsing transport text.
pub(crate) fn is_task_not_found(error: &(impl std::fmt::Display + ?Sized)) -> bool {
    let s = error.to_string();
    s.contains("HTTP 404") || (s.contains("\"code\":\"not_found\"") && s.contains("task"))
}

/// Safely transition a task to `target`, handling idempotent no-ops and
/// bridging known-disallowed direct transitions. See module docs for the
/// exact rules.
pub(crate) async fn safe_transition(
    domain: &dyn DomainApi,
    task_id: &str,
    target: &str,
) -> Result<TransitionOutcome, TransitionError> {
    let current = match domain.get_task(task_id, None).await {
        Ok(t) => t.status,
        Err(e) => {
            if is_task_not_found(&e) {
                return Ok(TransitionOutcome::LocalOnlyMissing);
            }
            return Err(TransitionError::LookupFailed {
                task_id: task_id.to_string(),
                target: target.to_string(),
                source: e,
            });
        }
    };

    if current == target {
        return Ok(TransitionOutcome::AlreadyInTarget);
    }

    if let Some(intermediate) = bridge_for(&current, target) {
        if let Err(e) = domain.transition_task(task_id, intermediate, None).await {
            // The final transition is authoritative; a failed bridge
            // can be a stale read or a concurrently applied transition.
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
        .map(|_| TransitionOutcome::Applied)
        .map_err(|e| TransitionError::TransitionFailed {
            task_id: task_id.to_string(),
            target: target.to_string(),
            source: e,
        })
}

/// Map of known-disallowed direct transitions to the intermediate
/// status that storage requires before the target transition.
fn bridge_for(current: &str, target: &str) -> Option<&'static str> {
    match (current, target) {
        ("pending", "in_progress") => Some("ready"),
        ("failed", "in_progress") => Some("ready"),
        ("ready", "failed") => Some("in_progress"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use aura_tools::domain_tools::{
        CreateSessionParams, MessageDescriptor, ProjectDescriptor, ProjectUpdate,
        SaveMessageParams, SessionDescriptor, SpecDescriptor, TaskDescriptor, TaskUpdate,
    };
    use std::sync::Mutex;

    /// Minimal `DomainApi` double that records every `transition_task`
    /// call and lets each test drive the status machine via
    /// `reject_transition`. Only the two methods used by
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
        // When set, `get_task` returns this error message, and
        // `transition_task` returns the same message. Lets tests
        // simulate the storage 404 responses the harness sees for
        // local-only task ids.
        fail_with: Mutex<Option<String>>,
    }

    impl FakeDomain {
        fn new(initial: &str) -> Self {
            Self {
                status: Mutex::new(initial.to_string()),
                reject: Mutex::new(Vec::new()),
                calls: Mutex::new(Vec::new()),
                fail_get: Mutex::new(false),
                fail_with: Mutex::new(None),
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
        fn set_fail_with(&self, msg: &str) {
            *self.fail_with.lock().unwrap() = Some(msg.to_string());
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
            if let Some(msg) = self.fail_with.lock().unwrap().clone() {
                return Err(anyhow::anyhow!(msg));
            }
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
        async fn get_task(&self, task_id: &str, _: Option<&str>) -> anyhow::Result<TaskDescriptor> {
            if let Some(msg) = self.fail_with.lock().unwrap().clone() {
                return Err(anyhow::anyhow!(msg));
            }
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
        async fn get_project(&self, _: &str, _: Option<&str>) -> anyhow::Result<ProjectDescriptor> {
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
        async fn list_messages(&self, _: &str, _: &str) -> anyhow::Result<Vec<MessageDescriptor>> {
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
        async fn get_active_session(&self, _: &str) -> anyhow::Result<Option<SessionDescriptor>> {
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
    }

    #[tokio::test]
    async fn skips_noop_transition_when_already_in_target_state() {
        let domain = FakeDomain::new("done");
        domain.reject_transition("done", "done");

        let outcome = safe_transition(&domain, "t1", "done").await.expect("skip");

        assert_eq!(outcome, TransitionOutcome::AlreadyInTarget);
        assert!(
            domain.calls().is_empty(),
            "safe_transition should not call transition_task when already in target; got {:?}",
            domain.calls()
        );
    }

    #[tokio::test]
    async fn bridges_pending_to_in_progress_via_ready() {
        let domain = FakeDomain::new("pending");
        domain.reject_transition("pending", "in_progress");

        let outcome = safe_transition(&domain, "t-pending", "in_progress")
            .await
            .expect("bridged");

        assert_eq!(outcome, TransitionOutcome::Applied);
        assert_eq!(
            domain.calls(),
            vec!["ready", "in_progress"],
            "expected pending→ready→in_progress bridge"
        );
    }

    #[tokio::test]
    async fn bridges_failed_to_in_progress_via_ready() {
        let domain = FakeDomain::new("failed");
        domain.reject_transition("failed", "in_progress");

        let outcome = safe_transition(&domain, "t2", "in_progress")
            .await
            .expect("bridged");

        assert_eq!(outcome, TransitionOutcome::Applied);
        assert_eq!(
            domain.calls(),
            vec!["ready", "in_progress"],
            "expected failed→ready→in_progress bridge"
        );
    }

    #[tokio::test]
    async fn passes_through_ordinary_transition_unchanged() {
        let domain = FakeDomain::new("in_progress");

        let outcome = safe_transition(&domain, "t3", "done").await.expect("ok");

        assert_eq!(outcome, TransitionOutcome::Applied);
        assert_eq!(domain.calls(), vec!["done"]);
    }

    #[tokio::test]
    async fn lookup_failure_does_not_fall_back_to_direct_transition() {
        let domain = FakeDomain::new("in_progress");
        domain.set_get_failure(true);

        let err = safe_transition(&domain, "t4", "done")
            .await
            .expect_err("lookup failure must surface");

        assert!(
            matches!(err, TransitionError::LookupFailed { .. }),
            "expected lookup error, got {err:?}"
        );
        assert!(
            domain.calls().is_empty(),
            "transition_task must not be called after non-404 lookup failure; got {:?}",
            domain.calls()
        );
    }

    #[tokio::test]
    async fn returns_local_only_outcome_when_get_task_returns_404() {
        let domain = FakeDomain::new("in_progress");
        domain.set_fail_with(
            "HTTP 404 Not Found: {\"error\":\"task not found\",\"code\":\"not_found\"}",
        );

        let outcome = safe_transition(&domain, "t-local-only", "done")
            .await
            .expect("404 must be treated as a no-op skip");

        assert_eq!(outcome, TransitionOutcome::LocalOnlyMissing);
        assert!(
            domain.calls().is_empty(),
            "transition_task must NOT be called after a 404 lookup; got {:?}",
            domain.calls()
        );
    }

    #[test]
    fn is_task_not_found_matches_storage_404() {
        // Real `HttpDomainApi` 404 envelope (see
        // `crates/aura-runtime/src/domain.rs::api_get`).
        let err = anyhow::anyhow!(
            "HTTP 404 Not Found: {{\"error\":\"task not found\",\"code\":\"not_found\",\"details\":null}}"
        );
        assert!(is_task_not_found(&err));
    }

    #[test]
    fn is_task_not_found_matches_lowercase_code_envelope() {
        // Defensive: detect not-found purely from the JSON envelope
        // even if the upstream `HTTP {status}` prefix changes shape.
        let err = anyhow::anyhow!(
            "request failed: {{\"error\":\"task not found\",\"code\":\"not_found\"}}"
        );
        assert!(is_task_not_found(&err));
    }

    #[test]
    fn is_task_not_found_rejects_unrelated_errors() {
        // 400 / 500 / cloudflare must keep reaching their WARN sites.
        let err400 = anyhow::anyhow!("HTTP 400 Bad Request: invalid status transition");
        let err500 = anyhow::anyhow!("HTTP 500 Internal Server Error: db timeout");
        let cloudflare = anyhow::anyhow!("HTTP 403 Forbidden: <!DOCTYPE html>...Cloudflare...");
        assert!(!is_task_not_found(&err400));
        assert!(!is_task_not_found(&err500));
        assert!(!is_task_not_found(&cloudflare));
    }
}
