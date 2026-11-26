
use super::*;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use anyhow::anyhow;
use async_trait::async_trait;
use aura_exec_tools::domain_tools::{
    CreateSessionParams, DomainApi, MessageDescriptor, ProjectDescriptor, ProjectUpdate,
    SaveMessageParams, SessionDescriptor, SpecDescriptor, TaskDescriptor, TaskUpdate,
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
    ) -> anyhow::Result<Vec<SpecDescriptor>> {
        unimplemented!("RecordingDomain")
    }
    async fn get_spec(&self, _spec_id: &str, _jwt: Option<&str>) -> anyhow::Result<SpecDescriptor> {
        unimplemented!("RecordingDomain")
    }
    async fn create_spec(
        &self,
        _p: &str,
        _t: &str,
        _c: &str,
        _o: u32,
        _j: Option<&str>,
    ) -> anyhow::Result<SpecDescriptor> {
        unimplemented!("RecordingDomain")
    }
    async fn update_spec(
        &self,
        _id: &str,
        _t: Option<&str>,
        _c: Option<&str>,
        _if_match: Option<&str>,
        _j: Option<&str>,
    ) -> anyhow::Result<SpecDescriptor> {
        unimplemented!("RecordingDomain")
    }
    async fn delete_spec(&self, _id: &str, _j: Option<&str>) -> anyhow::Result<()> {
        unimplemented!("RecordingDomain")
    }
    async fn list_tasks(
        &self,
        _p: &str,
        _s: Option<&str>,
        _j: Option<&str>,
    ) -> anyhow::Result<Vec<TaskDescriptor>> {
        unimplemented!("RecordingDomain")
    }
    async fn create_task(
        &self,
        _p: &str,
        _s: &str,
        _t: &str,
        _d: &str,
        _deps: &[String],
        _o: u32,
        _j: Option<&str>,
    ) -> anyhow::Result<TaskDescriptor> {
        unimplemented!("RecordingDomain")
    }
    async fn update_task(
        &self,
        id: &str,
        updates: TaskUpdate,
        _jwt: Option<&str>,
    ) -> anyhow::Result<TaskDescriptor> {
        self.updates
            .lock()
            .unwrap()
            .push((id.to_string(), updates.clone()));
        if self.update_should_fail {
            return Err(anyhow!("simulated update_task failure"));
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
    async fn delete_task(&self, _id: &str, _j: Option<&str>) -> anyhow::Result<()> {
        unimplemented!("RecordingDomain")
    }
    async fn transition_task(
        &self,
        _id: &str,
        _s: &str,
        _j: Option<&str>,
    ) -> anyhow::Result<TaskDescriptor> {
        unimplemented!("RecordingDomain")
    }
    async fn claim_next_task(
        &self,
        _p: &str,
        _a: &str,
        _j: Option<&str>,
    ) -> anyhow::Result<Option<TaskDescriptor>> {
        unimplemented!("RecordingDomain")
    }
    async fn get_task(&self, _id: &str, _j: Option<&str>) -> anyhow::Result<TaskDescriptor> {
        unimplemented!("RecordingDomain")
    }
    async fn get_project(&self, _p: &str, _j: Option<&str>) -> anyhow::Result<ProjectDescriptor> {
        unimplemented!("RecordingDomain")
    }
    async fn update_project(
        &self,
        _p: &str,
        _u: ProjectUpdate,
        _j: Option<&str>,
    ) -> anyhow::Result<ProjectDescriptor> {
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
    ) -> anyhow::Result<serde_json::Value> {
        unimplemented!("RecordingDomain")
    }
    async fn list_logs(
        &self,
        _p: &str,
        _l: Option<&str>,
        _n: Option<u64>,
        _j: Option<&str>,
    ) -> anyhow::Result<serde_json::Value> {
        unimplemented!("RecordingDomain")
    }
    async fn get_project_stats(
        &self,
        _p: &str,
        _j: Option<&str>,
    ) -> anyhow::Result<serde_json::Value> {
        unimplemented!("RecordingDomain")
    }
    async fn list_messages(&self, _p: &str, _i: &str) -> anyhow::Result<Vec<MessageDescriptor>> {
        unimplemented!("RecordingDomain")
    }
    async fn save_message(&self, _p: SaveMessageParams) -> anyhow::Result<()> {
        unimplemented!("RecordingDomain")
    }
    async fn create_session(&self, _p: CreateSessionParams) -> anyhow::Result<SessionDescriptor> {
        unimplemented!("RecordingDomain")
    }
    async fn get_active_session(&self, _i: &str) -> anyhow::Result<Option<SessionDescriptor>> {
        unimplemented!("RecordingDomain")
    }
    async fn network_api_call(
        &self,
        _m: &str,
        _p: &str,
        _b: Option<&serde_json::Value>,
        _j: Option<&str>,
    ) -> anyhow::Result<String> {
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

#[tokio::test]
async fn skips_when_marker_present() {
    let domain = RecordingDomain::default();
    let provider = CountingProvider::with_text("should never run");
    let (tx, mut rx) = mpsc::channel::<AutomatonEvent>(8);
    let spec = sample_spec();
    let task = sample_task("<!-- aura-refined:v1 -->\n## Refined Description\nalready done");

    let out = refine_task_description(&domain, &provider, "test-model", &spec, &task, Some(&tx))
        .await
        .expect("marker short-circuit must succeed");

    assert_eq!(
        out.description, task.description,
        "marker short-circuit must return the input description unchanged"
    );
    assert_eq!(
        provider.calls(),
        0,
        "marker short-circuit must not call the provider"
    );
    assert!(
        domain.updates().is_empty(),
        "marker short-circuit must not call update_task"
    );
    assert!(
        drain(&mut rx).is_empty(),
        "marker short-circuit must emit no events"
    );
}

#[tokio::test]
async fn falls_back_on_provider_error() {
    let domain = RecordingDomain::default();
    let provider = CountingProvider::with_error();
    let (tx, mut rx) = mpsc::channel::<AutomatonEvent>(8);
    let spec = sample_spec();
    let task = sample_task("Make the tokens rotate every 15 minutes.");

    let out = refine_task_description(&domain, &provider, "test-model", &spec, &task, Some(&tx))
        .await
        .expect("provider failure must not error the caller");

    assert_eq!(
        out.description, task.description,
        "provider failure must return the original description"
    );
    assert!(
        domain.updates().is_empty(),
        "provider failure must not persist anything"
    );

    let events = drain(&mut rx);
    let mut saw_refining = false;
    let mut saw_log = false;
    let mut saw_refined = false;
    for e in &events {
        match e {
            AutomatonEvent::TaskDescriptionRefining { task_id } => {
                assert_eq!(task_id, "task-1");
                saw_refining = true;
            }
            AutomatonEvent::LogLine { message } => {
                assert!(
                    message.contains("task-1") && message.contains("refinement failed"),
                    "LogLine must carry the failure context, got: {message}"
                );
                saw_log = true;
            }
            AutomatonEvent::TaskDescriptionRefined { .. } => {
                saw_refined = true;
            }
            other => panic!("unexpected event on provider-failure path: {other:?}"),
        }
    }
    assert!(
        saw_refining,
        "refining event must still fire before the call"
    );
    assert!(saw_log, "exactly one LogLine must capture the failure");
    assert!(
        !saw_refined,
        "the success event must NOT fire when the provider rejected the call"
    );
}

#[tokio::test]
async fn persists_refined_description() {
    let domain = RecordingDomain::default();
    let provider = CountingProvider::with_text(
        "Rotate refresh tokens every 15 minutes and revoke prior keys.",
    );
    let (tx, mut rx) = mpsc::channel::<AutomatonEvent>(8);
    let spec = sample_spec();
    let task = sample_task("Rotate tokens.");

    let out = refine_task_description(&domain, &provider, "test-model", &spec, &task, Some(&tx))
        .await
        .expect("happy path must succeed");

    assert!(
        out.description.starts_with(REFINED_MARKER),
        "returned description must carry the marker, got: {}",
        out.description
    );
    assert!(
        out.description.contains("## Original Task"),
        "returned description must preserve the original task block"
    );
    assert!(
        out.description.contains("> Rotate tokens."),
        "original description must be quoted line-by-line"
    );
    assert!(
        out.description
            .contains("Rotate refresh tokens every 15 minutes"),
        "refined model output must be embedded in the body"
    );

    let updates = domain.updates();
    assert_eq!(updates.len(), 1, "update_task must be called exactly once");
    let (id, update) = &updates[0];
    assert_eq!(id, "task-1");
    let persisted = update
        .description
        .as_deref()
        .expect("update payload must carry the new description");
    assert!(persisted.starts_with(REFINED_MARKER));
    assert!(persisted.contains("## Original Task"));

    let events = drain(&mut rx);
    let mut saw_refining = false;
    let mut saw_refined = false;
    for e in &events {
        match e {
            AutomatonEvent::TaskDescriptionRefining { task_id } => {
                assert_eq!(task_id, "task-1");
                saw_refining = true;
            }
            AutomatonEvent::TaskDescriptionRefined { task_id } => {
                assert_eq!(task_id, "task-1");
                saw_refined = true;
            }
            AutomatonEvent::LogLine { message } => {
                panic!("happy path must not emit a LogLine, got: {message}")
            }
            other => panic!("unexpected event on happy path: {other:?}"),
        }
    }
    assert!(saw_refining, "TaskDescriptionRefining must fire");
    assert!(saw_refined, "TaskDescriptionRefined must fire on success");
}

#[tokio::test]
async fn empty_provider_output_falls_back_to_original() {
    // A whitespace-only model response is functionally indistinguishable
    // from a failure — the original description is strictly more
    // informative. Lock in the fall-through.
    let domain = RecordingDomain::default();
    let provider = CountingProvider::with_text("   \n\t ");
    let (tx, mut rx) = mpsc::channel::<AutomatonEvent>(8);
    let spec = sample_spec();
    let task = sample_task("Rotate tokens.");

    let out = refine_task_description(&domain, &provider, "test-model", &spec, &task, Some(&tx))
        .await
        .expect("empty provider output must not error the caller");

    assert_eq!(out.description, task.description);
    assert!(
        domain.updates().is_empty(),
        "empty refinement must not persist anything"
    );
    let events = drain(&mut rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AutomatonEvent::LogLine { .. })),
        "empty refinement must emit a LogLine"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AutomatonEvent::TaskDescriptionRefined { .. })),
        "empty refinement must NOT emit the success event"
    );
}

#[tokio::test]
async fn falls_back_on_update_task_error() {
    let domain = RecordingDomain {
        update_should_fail: true,
        ..Default::default()
    };
    let provider = CountingProvider::with_text("Refined body.");
    let (tx, mut rx) = mpsc::channel::<AutomatonEvent>(8);
    let spec = sample_spec();
    let task = sample_task("Rotate tokens.");

    let out = refine_task_description(&domain, &provider, "test-model", &spec, &task, Some(&tx))
        .await
        .expect("update_task failure must not error the caller");

    assert_eq!(out.description, task.description);
    assert_eq!(
        provider.calls(),
        1,
        "provider must still have been called once"
    );
    assert_eq!(
        domain.updates().len(),
        1,
        "update_task is still attempted exactly once"
    );

    let events = drain(&mut rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AutomatonEvent::LogLine { .. })),
        "update_task failure must emit a LogLine"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AutomatonEvent::TaskDescriptionRefined { .. })),
        "update_task failure must NOT emit the success event"
    );
}

#[tokio::test]
async fn works_without_event_channel() {
    // event_tx = None must not panic and must still run the
    // provider + update_task path.
    let domain = RecordingDomain::default();
    let provider = CountingProvider::with_text("Refined body.");
    let spec = sample_spec();
    let task = sample_task("Rotate tokens.");

    let out = refine_task_description(&domain, &provider, "test-model", &spec, &task, None)
        .await
        .expect("None event_tx must work");

    assert_eq!(provider.calls(), 1);
    assert_eq!(domain.updates().len(), 1);
    assert!(out.description.starts_with(REFINED_MARKER));
}

#[test]
fn assemble_refined_body_quotes_multiline_descriptions() {
    let task = sample_task("Line one.\nLine two.\nLine three.");
    let body = assemble_refined_body("Refined body.", &task);
    assert!(body.starts_with(REFINED_MARKER));
    assert!(body.contains("## Refined Description\nRefined body."));
    assert!(body.contains("> Line one."));
    assert!(body.contains("> Line two."));
    assert!(body.contains("> Line three."));
}

/// Phase 7 contract: after Phase 7 dropped the dev-loop
/// `build_retry_note.is_none()` outer gate, the
/// [`REFINED_MARKER`] short-circuit is the single authoritative
/// idempotency check. This regression test simulates the
/// build-retry second pass directly — call once, take the
/// persisted refined description, call again with that body —
/// and asserts the provider is invoked exactly once.
///
/// If a future patch reintroduces the redundant outer gate or
/// removes the marker, this test will fail loudly because the
/// second call will issue a second provider request.
#[tokio::test]
async fn refinement_runs_at_most_once_per_task_across_retries() {
    let domain = RecordingDomain::default();
    let provider = CountingProvider::with_text("Refined: rotate every 15m.");
    let spec = sample_spec();
    let first = sample_task("Rotate tokens.");

    let after_first =
        refine_task_description(&domain, &provider, "test-model", &spec, &first, None)
            .await
            .expect("first refinement must succeed");

    assert_eq!(provider.calls(), 1, "first pass must call the provider");
    assert!(
        after_first.description.starts_with(REFINED_MARKER),
        "first pass must persist the marker"
    );

    // Second pass emulates a build-retry: re-enter
    // `refine_task_description` with the now-marker-prefixed
    // description (this is what the dev-loop would do when
    // re-claiming the same task with a build_retry_note).
    let after_second =
        refine_task_description(&domain, &provider, "test-model", &spec, &after_first, None)
            .await
            .expect("second refinement must succeed via short-circuit");

    assert_eq!(
        provider.calls(),
        1,
        "second pass must NOT call the provider — the marker is the only gate, \
             so re-entry on a refined body short-circuits silently"
    );
    assert_eq!(
        after_second.description, after_first.description,
        "marker short-circuit must return the input description unchanged"
    );
    assert_eq!(
        domain.updates().len(),
        1,
        "update_task must have been called only on the first pass"
    );
}
