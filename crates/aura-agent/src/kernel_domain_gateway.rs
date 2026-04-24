//! Kernel-mediated gateway for [`DomainApi`].
//!
//! [`KernelDomainGateway`] wraps an `Arc<dyn DomainApi>` and implements
//! the same trait. Read-only methods (`list_*`, `get_*`) are passed
//! through directly — they are declared exceptions per
//! `docs/invariants.md` §1. Every mutating method records a pre-call
//! "request snapshot" and a post-call "response" `RecordEntry` by
//! routing a [`Transaction`] with [`TransactionType::System`] and
//! [`SystemKind::DomainMutation`] through [`Kernel::process_direct`].
//!
//! The gateway satisfies Invariant §2 ("every state change is a
//! transaction") and §8 ("gateway transparency") for the domain API
//! surface consumed by automatons.
//!
//! ## Error handling
//!
//! - The HTTP timeout enforced by the concrete `DomainApi`
//!   implementation (`HttpDomainApi`) still applies because we call
//!   through the inner `Arc<dyn DomainApi>` directly.
//! - Failures from the kernel's `process_direct` call (e.g. store
//!   corruption) are logged at `error!` level but do not mask the
//!   underlying domain error on the mutation's result value.

use std::sync::Arc;

use async_trait::async_trait;
use aura_core::{SystemKind, Transaction, TransactionType};
use aura_kernel::Kernel;
use aura_tools::domain_tools::{
    CreateSessionParams, DomainApi, MessageDescriptor, ProjectDescriptor, ProjectUpdate,
    SaveMessageParams, SessionDescriptor, SpecDescriptor, TaskDescriptor, TaskUpdate,
};
use serde_json::{json, Value};
use tracing::error;

/// Errors emitted by the gateway when it fails to construct or submit
/// the mandatory `System`/`DomainMutation` record entries. These are
/// logged but do not replace the original mutation result; they exist
/// so internal call sites (tests in particular) can distinguish a
/// store-layer failure from a transport failure.
#[derive(Debug, thiserror::Error)]
pub enum KernelDomainGatewayError {
    /// The kernel refused to append the recording transaction.
    #[error("kernel recording failed: {0}")]
    Kernel(String),
    /// Serialization of the recording payload failed.
    #[error("recording payload serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// Gateway implementing [`DomainApi`] by routing every mutation
/// through [`Kernel::process_direct`] so the kernel's record log
/// captures the request snapshot and response outcome.
pub struct KernelDomainGateway {
    inner: Arc<dyn DomainApi>,
    kernel: Arc<Kernel>,
}

impl KernelDomainGateway {
    /// Wrap `inner` so every mutating call first records a request
    /// entry and then records the response (success or failure).
    #[must_use]
    pub fn new(inner: Arc<dyn DomainApi>, kernel: Arc<Kernel>) -> Self {
        Self { inner, kernel }
    }

    /// Record a "request" entry before an outbound mutating call.
    ///
    /// Returns `()` on success; errors are logged at `error!` so the
    /// caller can still attempt the outbound call. We deliberately do
    /// not propagate the recording error: per §3 / §2, a failure to
    /// record is not a reason to refuse the primary operation, but
    /// the underlying store failure is still surfaced via `tracing`
    /// so operators can detect silent drift.
    async fn record_request(&self, method: &'static str, args: Value) {
        let payload = json!({
            "system_kind": SystemKind::DomainMutation,
            "phase": "request",
            "method": method,
            "args": args,
        });
        let bytes = match serde_json::to_vec(&payload) {
            Ok(b) => b,
            Err(e) => {
                error!(method, error = %e, "KernelDomainGateway: failed to serialize request snapshot");
                return;
            }
        };
        let tx =
            Transaction::new_chained(self.kernel.agent_id, TransactionType::System, bytes, None);
        if let Err(e) = self.kernel.process_direct(tx).await {
            error!(method, error = %e, "KernelDomainGateway: failed to record domain mutation request");
        }
    }

    /// Record a "response" entry after an outbound mutating call.
    ///
    /// `ok` carries whether the inner call succeeded; when it failed
    /// the error message is captured verbatim in the payload.
    async fn record_response(&self, method: &'static str, ok: bool, error_msg: Option<String>) {
        let payload = json!({
            "system_kind": SystemKind::DomainMutation,
            "phase": "response",
            "method": method,
            "status": if ok { "ok" } else { "error" },
            "error": error_msg,
        });
        let bytes = match serde_json::to_vec(&payload) {
            Ok(b) => b,
            Err(e) => {
                error!(method, error = %e, "KernelDomainGateway: failed to serialize response snapshot");
                return;
            }
        };
        let tx =
            Transaction::new_chained(self.kernel.agent_id, TransactionType::System, bytes, None);
        if let Err(e) = self.kernel.process_direct(tx).await {
            error!(method, error = %e, "KernelDomainGateway: failed to record domain mutation response");
        }
    }
}

/// Helper macro: brackets an `inner` mutating call with pre/post
/// record entries. Returns the inner result verbatim.
macro_rules! with_recording {
    ($self:ident, $method:expr, $args:expr, $call:expr) => {{
        let method: &'static str = $method;
        $self.record_request(method, $args).await;
        let result = $call.await;
        let (ok, err_msg) = match &result {
            Ok(_) => (true, None),
            Err(e) => (false, Some(e.to_string())),
        };
        $self.record_response(method, ok, err_msg).await;
        result
    }};
}

/// Helper macro for mutating calls returning `anyhow::Result<()>`.
macro_rules! with_recording_unit {
    ($self:ident, $method:expr, $args:expr, $call:expr) => {{
        let method: &'static str = $method;
        $self.record_request(method, $args).await;
        let result = $call.await;
        let (ok, err_msg) = match &result {
            Ok(()) => (true, None),
            Err(e) => (false, Some(e.to_string())),
        };
        $self.record_response(method, ok, err_msg).await;
        result
    }};
}

#[async_trait]
impl DomainApi for KernelDomainGateway {
    // --- Specs ----------------------------------------------------------
    async fn list_specs(
        &self,
        project_id: &str,
        jwt: Option<&str>,
    ) -> anyhow::Result<Vec<SpecDescriptor>> {
        self.inner.list_specs(project_id, jwt).await
    }

    async fn get_spec(&self, spec_id: &str, jwt: Option<&str>) -> anyhow::Result<SpecDescriptor> {
        self.inner.get_spec(spec_id, jwt).await
    }

    async fn create_spec(
        &self,
        project_id: &str,
        title: &str,
        content: &str,
        order: u32,
        jwt: Option<&str>,
    ) -> anyhow::Result<SpecDescriptor> {
        with_recording!(
            self,
            "create_spec",
            json!({
                "project_id": project_id,
                "title": title,
                "order": order,
                "content_bytes": content.len(),
            }),
            self.inner
                .create_spec(project_id, title, content, order, jwt)
        )
    }

    async fn update_spec(
        &self,
        spec_id: &str,
        title: Option<&str>,
        content: Option<&str>,
        jwt: Option<&str>,
    ) -> anyhow::Result<SpecDescriptor> {
        with_recording!(
            self,
            "update_spec",
            json!({
                "spec_id": spec_id,
                "title_set": title.is_some(),
                "content_bytes": content.map(str::len),
            }),
            self.inner.update_spec(spec_id, title, content, jwt)
        )
    }

    async fn delete_spec(&self, spec_id: &str, jwt: Option<&str>) -> anyhow::Result<()> {
        with_recording_unit!(
            self,
            "delete_spec",
            json!({ "spec_id": spec_id }),
            self.inner.delete_spec(spec_id, jwt)
        )
    }

    // --- Tasks ----------------------------------------------------------
    async fn list_tasks(
        &self,
        project_id: &str,
        spec_id: Option<&str>,
        jwt: Option<&str>,
    ) -> anyhow::Result<Vec<TaskDescriptor>> {
        self.inner.list_tasks(project_id, spec_id, jwt).await
    }

    async fn create_task(
        &self,
        project_id: &str,
        spec_id: &str,
        title: &str,
        description: &str,
        dependencies: &[String],
        order: u32,
        jwt: Option<&str>,
    ) -> anyhow::Result<TaskDescriptor> {
        with_recording!(
            self,
            "create_task",
            json!({
                "project_id": project_id,
                "spec_id": spec_id,
                "title": title,
                "description_bytes": description.len(),
                "dependencies": dependencies,
                "order": order,
            }),
            self.inner.create_task(
                project_id,
                spec_id,
                title,
                description,
                dependencies,
                order,
                jwt
            )
        )
    }

    async fn update_task(
        &self,
        task_id: &str,
        updates: TaskUpdate,
        jwt: Option<&str>,
    ) -> anyhow::Result<TaskDescriptor> {
        let args = json!({
            "task_id": task_id,
            "updates": {
                "title_set": updates.title.is_some(),
                "description_set": updates.description.is_some(),
                "status_set": updates.status.is_some(),
            },
        });
        with_recording!(
            self,
            "update_task",
            args,
            self.inner.update_task(task_id, updates, jwt)
        )
    }

    async fn delete_task(&self, task_id: &str, jwt: Option<&str>) -> anyhow::Result<()> {
        with_recording_unit!(
            self,
            "delete_task",
            json!({ "task_id": task_id }),
            self.inner.delete_task(task_id, jwt)
        )
    }

    async fn transition_task(
        &self,
        task_id: &str,
        status: &str,
        jwt: Option<&str>,
    ) -> anyhow::Result<TaskDescriptor> {
        with_recording!(
            self,
            "transition_task",
            json!({ "task_id": task_id, "status": status }),
            self.inner.transition_task(task_id, status, jwt)
        )
    }

    async fn claim_next_task(
        &self,
        project_id: &str,
        agent_id: &str,
        jwt: Option<&str>,
    ) -> anyhow::Result<Option<TaskDescriptor>> {
        with_recording!(
            self,
            "claim_next_task",
            json!({ "project_id": project_id, "agent_id": agent_id }),
            self.inner.claim_next_task(project_id, agent_id, jwt)
        )
    }

    async fn get_task(&self, task_id: &str, jwt: Option<&str>) -> anyhow::Result<TaskDescriptor> {
        self.inner.get_task(task_id, jwt).await
    }

    // --- Project --------------------------------------------------------
    async fn get_project(
        &self,
        project_id: &str,
        jwt: Option<&str>,
    ) -> anyhow::Result<ProjectDescriptor> {
        self.inner.get_project(project_id, jwt).await
    }

    async fn update_project(
        &self,
        project_id: &str,
        updates: ProjectUpdate,
        jwt: Option<&str>,
    ) -> anyhow::Result<ProjectDescriptor> {
        let args = json!({
            "project_id": project_id,
            "updates": {
                "name_set": updates.name.is_some(),
                "description_set": updates.description.is_some(),
                "tech_stack_set": updates.tech_stack.is_some(),
                "build_command_set": updates.build_command.is_some(),
                "test_command_set": updates.test_command.is_some(),
            },
        });
        with_recording!(
            self,
            "update_project",
            args,
            self.inner.update_project(project_id, updates, jwt)
        )
    }

    // --- Storage (logs, stats) -----------------------------------------
    async fn create_log(
        &self,
        project_id: &str,
        message: &str,
        level: &str,
        agent_id: Option<&str>,
        metadata: Option<&serde_json::Value>,
        jwt: Option<&str>,
    ) -> anyhow::Result<serde_json::Value> {
        let args = json!({
            "project_id": project_id,
            "level": level,
            "message_bytes": message.len(),
            "agent_id": agent_id,
            "has_metadata": metadata.is_some(),
        });
        with_recording!(
            self,
            "create_log",
            args,
            self.inner
                .create_log(project_id, message, level, agent_id, metadata, jwt)
        )
    }

    async fn list_logs(
        &self,
        project_id: &str,
        level: Option<&str>,
        limit: Option<u64>,
        jwt: Option<&str>,
    ) -> anyhow::Result<serde_json::Value> {
        self.inner.list_logs(project_id, level, limit, jwt).await
    }

    async fn get_project_stats(
        &self,
        project_id: &str,
        jwt: Option<&str>,
    ) -> anyhow::Result<serde_json::Value> {
        self.inner.get_project_stats(project_id, jwt).await
    }

    // --- Messages -------------------------------------------------------
    async fn list_messages(
        &self,
        project_id: &str,
        instance_id: &str,
    ) -> anyhow::Result<Vec<MessageDescriptor>> {
        self.inner.list_messages(project_id, instance_id).await
    }

    async fn save_message(&self, params: SaveMessageParams) -> anyhow::Result<()> {
        let args = json!({
            "project_id": params.project_id,
            "instance_id": params.instance_id,
            "session_id": params.session_id,
            "role": params.role,
            "content_bytes": params.content.len(),
        });
        with_recording_unit!(self, "save_message", args, self.inner.save_message(params))
    }

    // --- Sessions -------------------------------------------------------
    async fn create_session(
        &self,
        params: CreateSessionParams,
    ) -> anyhow::Result<SessionDescriptor> {
        let args = json!({
            "instance_id": params.instance_id,
            "project_id": params.project_id,
            "model": params.model,
        });
        with_recording!(
            self,
            "create_session",
            args,
            self.inner.create_session(params)
        )
    }

    async fn get_active_session(
        &self,
        instance_id: &str,
    ) -> anyhow::Result<Option<SessionDescriptor>> {
        self.inner.get_active_session(instance_id).await
    }

    // --- Orbit / Network pass-through ----------------------------------
    //
    // These are generic verbs; classify via HTTP method. `GET` /
    // `HEAD` / `OPTIONS` are treated as read-only; everything else
    // routes through the kernel.
    async fn orbit_api_call(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
        jwt: Option<&str>,
    ) -> anyhow::Result<String> {
        if is_read_only_http_method(method) {
            return self.inner.orbit_api_call(method, path, body, jwt).await;
        }
        let args = json!({
            "method": method,
            "path": path,
            "has_body": body.is_some(),
        });
        with_recording!(
            self,
            "orbit_api_call",
            args,
            self.inner.orbit_api_call(method, path, body, jwt)
        )
    }

    fn orbit_url(&self) -> &str {
        self.inner.orbit_url()
    }

    async fn network_api_call(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
        jwt: Option<&str>,
    ) -> anyhow::Result<String> {
        if is_read_only_http_method(method) {
            return self.inner.network_api_call(method, path, body, jwt).await;
        }
        let args = json!({
            "method": method,
            "path": path,
            "has_body": body.is_some(),
        });
        with_recording!(
            self,
            "network_api_call",
            args,
            self.inner.network_api_call(method, path, body, jwt)
        )
    }
}

fn is_read_only_http_method(method: &str) -> bool {
    matches!(
        method.to_ascii_uppercase().as_str(),
        "GET" | "HEAD" | "OPTIONS"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core::{AgentId, TransactionType};
    use aura_kernel::{ExecutorRouter, KernelConfig};
    use aura_reasoner::{MockProvider, ModelProvider};
    use aura_store::{RocksStore, Store};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use tempfile::TempDir;

    // ---- Test double for `DomainApi` ---------------------------------

    #[derive(Default)]
    struct MockDomain {
        call_log: Mutex<Vec<&'static str>>,
        list_tasks_calls: AtomicUsize,
        fail_create_spec: bool,
    }

    impl MockDomain {
        fn new() -> Self {
            Self::default()
        }
        fn with_failing_create_spec() -> Self {
            Self {
                fail_create_spec: true,
                ..Self::default()
            }
        }
        fn record(&self, name: &'static str) {
            self.call_log.lock().unwrap().push(name);
        }
    }

    #[async_trait]
    impl DomainApi for MockDomain {
        async fn list_specs(
            &self,
            _project_id: &str,
            _jwt: Option<&str>,
        ) -> anyhow::Result<Vec<SpecDescriptor>> {
            self.record("list_specs");
            Ok(vec![])
        }
        async fn get_spec(
            &self,
            spec_id: &str,
            _jwt: Option<&str>,
        ) -> anyhow::Result<SpecDescriptor> {
            self.record("get_spec");
            Ok(SpecDescriptor {
                id: spec_id.to_string(),
                project_id: "p".to_string(),
                title: "t".to_string(),
                content: String::new(),
                order: 0,
                parent_id: None,
            })
        }
        async fn create_spec(
            &self,
            project_id: &str,
            title: &str,
            content: &str,
            order: u32,
            _jwt: Option<&str>,
        ) -> anyhow::Result<SpecDescriptor> {
            self.record("create_spec");
            if self.fail_create_spec {
                anyhow::bail!("simulated domain failure");
            }
            Ok(SpecDescriptor {
                id: "new-spec".to_string(),
                project_id: project_id.to_string(),
                title: title.to_string(),
                content: content.to_string(),
                order,
                parent_id: None,
            })
        }
        async fn update_spec(
            &self,
            spec_id: &str,
            _title: Option<&str>,
            _content: Option<&str>,
            _jwt: Option<&str>,
        ) -> anyhow::Result<SpecDescriptor> {
            self.record("update_spec");
            Ok(SpecDescriptor {
                id: spec_id.to_string(),
                project_id: "p".to_string(),
                title: "t".to_string(),
                content: String::new(),
                order: 0,
                parent_id: None,
            })
        }
        async fn delete_spec(&self, _spec_id: &str, _jwt: Option<&str>) -> anyhow::Result<()> {
            self.record("delete_spec");
            Ok(())
        }
        async fn list_tasks(
            &self,
            _project_id: &str,
            _spec_id: Option<&str>,
            _jwt: Option<&str>,
        ) -> anyhow::Result<Vec<TaskDescriptor>> {
            self.list_tasks_calls.fetch_add(1, Ordering::SeqCst);
            self.record("list_tasks");
            Ok(vec![])
        }
        async fn create_task(
            &self,
            project_id: &str,
            spec_id: &str,
            title: &str,
            description: &str,
            _dependencies: &[String],
            order: u32,
            _jwt: Option<&str>,
        ) -> anyhow::Result<TaskDescriptor> {
            self.record("create_task");
            Ok(TaskDescriptor {
                id: "t1".into(),
                spec_id: spec_id.into(),
                project_id: project_id.into(),
                title: title.into(),
                description: description.into(),
                status: "open".into(),
                dependencies: vec![],
                order,
            })
        }
        async fn update_task(
            &self,
            task_id: &str,
            _updates: TaskUpdate,
            _jwt: Option<&str>,
        ) -> anyhow::Result<TaskDescriptor> {
            self.record("update_task");
            Ok(TaskDescriptor {
                id: task_id.into(),
                spec_id: String::new(),
                project_id: String::new(),
                title: String::new(),
                description: String::new(),
                status: "open".into(),
                dependencies: vec![],
                order: 0,
            })
        }
        async fn delete_task(&self, _task_id: &str, _jwt: Option<&str>) -> anyhow::Result<()> {
            self.record("delete_task");
            Ok(())
        }
        async fn transition_task(
            &self,
            task_id: &str,
            status: &str,
            _jwt: Option<&str>,
        ) -> anyhow::Result<TaskDescriptor> {
            self.record("transition_task");
            Ok(TaskDescriptor {
                id: task_id.into(),
                spec_id: String::new(),
                project_id: String::new(),
                title: String::new(),
                description: String::new(),
                status: status.into(),
                dependencies: vec![],
                order: 0,
            })
        }
        async fn claim_next_task(
            &self,
            _project_id: &str,
            _agent_id: &str,
            _jwt: Option<&str>,
        ) -> anyhow::Result<Option<TaskDescriptor>> {
            self.record("claim_next_task");
            Ok(None)
        }
        async fn get_task(
            &self,
            task_id: &str,
            _jwt: Option<&str>,
        ) -> anyhow::Result<TaskDescriptor> {
            self.record("get_task");
            Ok(TaskDescriptor {
                id: task_id.into(),
                spec_id: String::new(),
                project_id: String::new(),
                title: String::new(),
                description: String::new(),
                status: "open".into(),
                dependencies: vec![],
                order: 0,
            })
        }
        async fn get_project(
            &self,
            project_id: &str,
            _jwt: Option<&str>,
        ) -> anyhow::Result<ProjectDescriptor> {
            self.record("get_project");
            Ok(ProjectDescriptor {
                id: project_id.into(),
                name: "p".into(),
                path: String::new(),
                description: None,
                tech_stack: None,
                build_command: None,
                test_command: None,
            })
        }
        async fn update_project(
            &self,
            project_id: &str,
            _updates: ProjectUpdate,
            _jwt: Option<&str>,
        ) -> anyhow::Result<ProjectDescriptor> {
            self.record("update_project");
            Ok(ProjectDescriptor {
                id: project_id.into(),
                name: "p".into(),
                path: String::new(),
                description: None,
                tech_stack: None,
                build_command: None,
                test_command: None,
            })
        }
        async fn create_log(
            &self,
            _project_id: &str,
            _message: &str,
            _level: &str,
            _agent_id: Option<&str>,
            _metadata: Option<&serde_json::Value>,
            _jwt: Option<&str>,
        ) -> anyhow::Result<serde_json::Value> {
            self.record("create_log");
            Ok(json!({ "ok": true }))
        }
        async fn list_logs(
            &self,
            _project_id: &str,
            _level: Option<&str>,
            _limit: Option<u64>,
            _jwt: Option<&str>,
        ) -> anyhow::Result<serde_json::Value> {
            self.record("list_logs");
            Ok(json!([]))
        }
        async fn get_project_stats(
            &self,
            _project_id: &str,
            _jwt: Option<&str>,
        ) -> anyhow::Result<serde_json::Value> {
            self.record("get_project_stats");
            Ok(json!({}))
        }
        async fn list_messages(
            &self,
            _project_id: &str,
            _instance_id: &str,
        ) -> anyhow::Result<Vec<MessageDescriptor>> {
            self.record("list_messages");
            Ok(vec![])
        }
        async fn save_message(&self, _params: SaveMessageParams) -> anyhow::Result<()> {
            self.record("save_message");
            Ok(())
        }
        async fn create_session(
            &self,
            params: CreateSessionParams,
        ) -> anyhow::Result<SessionDescriptor> {
            self.record("create_session");
            Ok(SessionDescriptor {
                id: "s1".into(),
                instance_id: params.instance_id,
                project_id: params.project_id,
                status: "active".into(),
            })
        }
        async fn get_active_session(
            &self,
            _instance_id: &str,
        ) -> anyhow::Result<Option<SessionDescriptor>> {
            self.record("get_active_session");
            Ok(None)
        }
        async fn orbit_api_call(
            &self,
            method: &str,
            _path: &str,
            _body: Option<&serde_json::Value>,
            _jwt: Option<&str>,
        ) -> anyhow::Result<String> {
            self.record("orbit_api_call");
            Ok(format!("orbit:{method}"))
        }
        async fn network_api_call(
            &self,
            method: &str,
            _path: &str,
            _body: Option<&serde_json::Value>,
            _jwt: Option<&str>,
        ) -> anyhow::Result<String> {
            self.record("network_api_call");
            Ok(format!("network:{method}"))
        }
    }

    fn build_kernel() -> (Arc<Kernel>, Arc<dyn Store>, TempDir, TempDir) {
        let db = TempDir::new().unwrap();
        let ws = TempDir::new().unwrap();
        let store: Arc<dyn Store> = Arc::new(RocksStore::open(db.path(), false).unwrap());
        let provider: Arc<dyn ModelProvider + Send + Sync> =
            Arc::new(MockProvider::simple_response("noop"));
        let cfg = KernelConfig {
            workspace_base: ws.path().to_path_buf(),
            ..KernelConfig::default()
        };
        let kernel = Arc::new(
            Kernel::new(
                store.clone(),
                provider,
                ExecutorRouter::new(),
                cfg,
                AgentId::generate(),
            )
            .unwrap(),
        );
        (kernel, store, db, ws)
    }

    fn count_domain_mutation_entries(store: &Arc<dyn Store>, kernel: &Kernel) -> Vec<Value> {
        let entries = store.scan_record(kernel.agent_id, 0, 256).unwrap();
        entries
            .into_iter()
            .filter(|e| e.tx.tx_type == TransactionType::System)
            .filter_map(|e| serde_json::from_slice::<Value>(&e.tx.payload).ok())
            .filter(|p| p.get("system_kind").and_then(Value::as_str) == Some("domain_mutation"))
            .collect()
    }

    #[tokio::test]
    async fn readonly_methods_passthrough_without_recording() {
        let (kernel, store, _db, _ws) = build_kernel();
        let inner = Arc::new(MockDomain::new());
        let gw = KernelDomainGateway::new(inner.clone(), kernel.clone());

        let _ = gw.list_tasks("p1", None, None).await.unwrap();
        let _ = gw.get_project("p1", None).await.unwrap();
        let _ = gw.list_specs("p1", None).await.unwrap();
        let _ = gw.get_spec("s1", None).await.unwrap();

        assert_eq!(inner.list_tasks_calls.load(Ordering::SeqCst), 1);

        let entries = count_domain_mutation_entries(&store, &kernel);
        assert!(
            entries.is_empty(),
            "read-only methods must not record DomainMutation entries, got: {entries:?}"
        );
    }

    #[tokio::test]
    async fn mutating_method_records_request_and_response_entries() {
        let (kernel, store, _db, _ws) = build_kernel();
        let inner = Arc::new(MockDomain::new());
        let gw = KernelDomainGateway::new(inner.clone(), kernel.clone());

        let spec = gw
            .create_spec("proj-42", "Title", "Body", 0, None)
            .await
            .expect("create_spec succeeds");
        assert_eq!(spec.id, "new-spec");

        let entries = count_domain_mutation_entries(&store, &kernel);
        assert_eq!(
            entries.len(),
            2,
            "expected request+response entries, got {entries:?}"
        );
        let phases: Vec<&str> = entries
            .iter()
            .filter_map(|p| p.get("phase").and_then(Value::as_str))
            .collect();
        assert_eq!(phases, vec!["request", "response"]);
        assert_eq!(entries[1].get("status").and_then(Value::as_str), Some("ok"));
        assert_eq!(
            entries[0].get("method").and_then(Value::as_str),
            Some("create_spec")
        );
    }

    #[tokio::test]
    async fn mutating_method_records_on_failure() {
        let (kernel, store, _db, _ws) = build_kernel();
        let inner = Arc::new(MockDomain::with_failing_create_spec());
        let gw = KernelDomainGateway::new(inner.clone(), kernel.clone());

        let err = gw
            .create_spec("proj-42", "Title", "Body", 0, None)
            .await
            .expect_err("create_spec must propagate domain failure");
        assert!(err.to_string().contains("simulated domain failure"));

        let entries = count_domain_mutation_entries(&store, &kernel);
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[1].get("status").and_then(Value::as_str),
            Some("error"),
            "second entry must carry status=error, got: {:?}",
            entries[1]
        );
        assert!(entries[1]
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .contains("simulated domain failure"));
    }

    #[tokio::test]
    async fn orbit_get_is_passthrough_post_is_recorded() {
        let (kernel, store, _db, _ws) = build_kernel();
        let inner = Arc::new(MockDomain::new());
        let gw = KernelDomainGateway::new(inner.clone(), kernel.clone());

        let _ = gw
            .orbit_api_call("GET", "/repos", None, None)
            .await
            .unwrap();
        let no_entries = count_domain_mutation_entries(&store, &kernel);
        assert!(no_entries.is_empty(), "GET must not record");

        let _ = gw
            .orbit_api_call("POST", "/repos", None, None)
            .await
            .unwrap();
        let entries = count_domain_mutation_entries(&store, &kernel);
        assert_eq!(entries.len(), 2);
    }
}
