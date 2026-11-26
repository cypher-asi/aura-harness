use super::*;

use std::sync::atomic::{AtomicUsize, Ordering};

use std::sync::Mutex;

use async_trait::async_trait;

use aura_exec_tools::domain_tools::{
    CreateSessionParams, DomainApi, DomainError, DomainResult, MessageDescriptor, NewTask,
    ProjectDescriptor, ProjectUpdate, SaveMessageParams, SessionDescriptor, SpecDescriptor,
    TaskDescriptor, TaskUpdate,
};

use aura_model::{
    ContentBlock, Message as ReasonerMessage, ModelRequest, ModelResponse, ProviderTrace,
    ReasonerError, Role, StopReason, Usage,
};

// ------------------------------------------------------------
// Mock ModelProvider that counts calls and lets the test inject
// either a canned text response or an error.
// ------------------------------------------------------------

struct CountingProvider {
    outcome: Mutex<Vec<Result<String, &'static str>>>,
    calls: AtomicUsize,
}

impl CountingProvider {
    fn with_text(text: &str) -> Self {
        Self {
            outcome: Mutex::new(vec![Ok(text.to_string())]),
            calls: AtomicUsize::new(0),
        }
    }

    fn with_error() -> Self {
        Self {
            outcome: Mutex::new(vec![Err("simulated provider failure")]),
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ModelProvider for CountingProvider {
    fn name(&self) -> &'static str {
        "test-counting-provider"
    }

    async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let next = {
            let mut q = self.outcome.lock().unwrap();
            if q.is_empty() {
                Ok("default refined body".to_string())
            } else {
                q.remove(0)
            }
        };
        match next {
            Ok(text) => Ok(ModelResponse {
                stop_reason: StopReason::EndTurn,
                message: ReasonerMessage {
                    role: Role::Assistant,
                    content: vec![ContentBlock::text(text)],
                },
                usage: Usage::new(10, 20),
                trace: ProviderTrace::new("test-model", 0),
                model_used: "test-model".to_string(),
            }),
            Err(msg) => Err(ReasonerError::Internal(msg.to_string())),
        }
    }

    async fn health_check(&self) -> bool {
        true
    }
}

// ------------------------------------------------------------
// Mock DomainApi: every method `unimplemented!()` except
// `update_task`, which records its inputs and returns a
// descriptor with the description we received. Mirrors the
// `UnusedDomain` pattern in
// `crates/surface/aura-node/src/automaton_bridge/tests.rs`.
// ------------------------------------------------------------

#[derive(Default)]
struct RecordingDomain {
    updates: Mutex<Vec<(String, TaskUpdate)>>,
    update_should_fail: bool,
}

impl RecordingDomain {
    fn updates(&self) -> Vec<(String, TaskUpdate)> {
        self.updates.lock().unwrap().clone()
    }
}

#[async_trait]
impl DomainApi for RecordingDomain {
    async fn list_specs(
        &self,
        _project_id: &str,
        _jwt: Option<&str>,
    ) -> DomainResult<Vec<SpecDescriptor>> {
        unimplemented!("RecordingDomain")
    }
    async fn get_spec(&self, _spec_id: &str, _jwt: Option<&str>) -> DomainResult<SpecDescriptor> {
        unimplemented!("RecordingDomain")
    }
    async fn create_spec(
        &self,
        _p: &str,
        _t: &str,
        _c: &str,
        _o: u32,
        _j: Option<&str>,
    ) -> DomainResult<SpecDescriptor> {
        unimplemented!("RecordingDomain")
    }
    async fn update_spec(
        &self,
        _id: &str,
        _t: Option<&str>,
        _c: Option<&str>,
        _if_match: Option<&str>,
        _j: Option<&str>,
    ) -> DomainResult<SpecDescriptor> {
        unimplemented!("RecordingDomain")
    }
    async fn delete_spec(&self, _id: &str, _j: Option<&str>) -> DomainResult<()> {
        unimplemented!("RecordingDomain")
    }
    async fn list_tasks(
        &self,
        _p: &str,
        _s: Option<&str>,
        _j: Option<&str>,
    ) -> DomainResult<Vec<TaskDescriptor>> {
        unimplemented!("RecordingDomain")
    }
    async fn create_task(
        &self,
        _task: NewTask<'_>,
        _j: Option<&str>,
    ) -> DomainResult<TaskDescriptor> {
        unimplemented!("RecordingDomain")
    }
    async fn update_task(
        &self,
        id: &str,
        updates: TaskUpdate,
        _jwt: Option<&str>,
    ) -> DomainResult<TaskDescriptor> {
        self.updates
            .lock()
            .unwrap()
            .push((id.to_string(), updates.clone()));
        if self.update_should_fail {
            return Err(DomainError::Status {
                status: 500,
                reason: Some("Internal Server Error"),
                body: "simulated update_task failure".to_string(),
            });
        }
        let new_desc = updates.description.clone().unwrap_or_default();
        Ok(TaskDescriptor {
            id: id.to_string(),
            spec_id: String::new(),
            project_id: String::new(),
            title: String::new(),
            description: new_desc,
            status: String::new(),
            dependencies: Vec::new(),
            order: 0,
        })
    }
    async fn delete_task(&self, _id: &str, _j: Option<&str>) -> DomainResult<()> {
        unimplemented!("RecordingDomain")
    }
    async fn transition_task(
        &self,
        _id: &str,
        _s: &str,
        _j: Option<&str>,
    ) -> DomainResult<TaskDescriptor> {
        unimplemented!("RecordingDomain")
    }
    async fn claim_next_task(
        &self,
        _p: &str,
        _a: &str,
        _j: Option<&str>,
    ) -> DomainResult<Option<TaskDescriptor>> {
        unimplemented!("RecordingDomain")
    }
    async fn get_task(&self, _id: &str, _j: Option<&str>) -> DomainResult<TaskDescriptor> {
        unimplemented!("RecordingDomain")
    }
    async fn get_project(&self, _p: &str, _j: Option<&str>) -> DomainResult<ProjectDescriptor> {
        unimplemented!("RecordingDomain")
    }
    async fn update_project(
        &self,
        _p: &str,
        _u: ProjectUpdate,
        _j: Option<&str>,
    ) -> DomainResult<ProjectDescriptor> {
        unimplemented!("RecordingDomain")
    }
    async fn create_log(
        &self,
        _p: &str,
        _m: &str,
        _l: &str,
        _a: Option<&str>,
        _md: Option<&serde_json::Value>,
        _j: Option<&str>,
    ) -> DomainResult<serde_json::Value> {
        unimplemented!("RecordingDomain")
    }
    async fn list_logs(
        &self,
        _p: &str,
        _l: Option<&str>,
        _n: Option<u64>,
        _j: Option<&str>,
    ) -> DomainResult<serde_json::Value> {
        unimplemented!("RecordingDomain")
    }
    async fn get_project_stats(
        &self,
        _p: &str,
        _j: Option<&str>,
    ) -> DomainResult<serde_json::Value> {
        unimplemented!("RecordingDomain")
    }
    async fn list_messages(&self, _p: &str, _i: &str) -> DomainResult<Vec<MessageDescriptor>> {
        unimplemented!("RecordingDomain")
    }
    async fn save_message(&self, _p: SaveMessageParams) -> DomainResult<()> {
        unimplemented!("RecordingDomain")
    }
    async fn create_session(&self, _p: CreateSessionParams) -> DomainResult<SessionDescriptor> {
        unimplemented!("RecordingDomain")
    }
    async fn get_active_session(&self, _i: &str) -> DomainResult<Option<SessionDescriptor>> {
        unimplemented!("RecordingDomain")
    }
    async fn network_api_call(
        &self,
        _m: &str,
        _p: &str,
        _b: Option<&serde_json::Value>,
        _j: Option<&str>,
    ) -> DomainResult<String> {
        unimplemented!("RecordingDomain")
    }
}

fn sample_spec() -> SpecDescriptor {
    SpecDescriptor {
        id: "spec-1".into(),
        project_id: "proj-1".into(),
        title: "Auth service".into(),
        content: "Implement token-based auth with rotating refresh tokens.".into(),
        order: 0,
        parent_id: None,
        content_hash: None,
    }
}

fn sample_task(description: &str) -> TaskDescriptor {
    TaskDescriptor {
        id: "task-1".into(),
        spec_id: "spec-1".into(),
        project_id: "proj-1".into(),
        title: "Wire refresh-token rotation".into(),
        description: description.into(),
        status: "ready".into(),
        dependencies: Vec::new(),
        order: 0,
    }
}

fn drain(rx: &mut mpsc::Receiver<AutomatonEvent>) -> Vec<AutomatonEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

mod outcomes;
mod retries;
