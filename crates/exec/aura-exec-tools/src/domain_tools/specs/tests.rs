use super::*;
use crate::domain_tools::api::{
    CreateSessionParams, DomainError, DomainResult, MessageDescriptor, NewTask, ProjectDescriptor,
    ProjectUpdate, SaveMessageParams, SessionDescriptor, TaskDescriptor, TaskUpdate,
};
use async_trait::async_trait;
use std::sync::Mutex;

/// What the methods these tests never call return.
fn unused<T>(operation: &'static str) -> DomainResult<T> {
    Err(DomainError::Unsupported { operation })
}

/// Minimal `DomainApi` test double specialised for the spec handlers.
/// Only the methods exercised by these tests are populated; everything
/// else bails so a misrouted call shows up loudly.
#[derive(Default)]
struct SpecMockApi {
    specs_to_return: Mutex<Vec<SpecDescriptor>>,
    single_spec: Mutex<Option<SpecDescriptor>>,
}

impl SpecMockApi {
    fn with_single_spec(spec: SpecDescriptor) -> Self {
        Self {
            specs_to_return: Mutex::new(vec![]),
            single_spec: Mutex::new(Some(spec)),
        }
    }
    fn with_listing(specs: Vec<SpecDescriptor>) -> Self {
        Self {
            specs_to_return: Mutex::new(specs),
            single_spec: Mutex::new(None),
        }
    }
}

#[async_trait]
impl DomainApi for SpecMockApi {
    async fn list_specs(
        &self,
        _project_id: &str,
        _jwt: Option<&str>,
    ) -> DomainResult<Vec<SpecDescriptor>> {
        Ok(self.specs_to_return.lock().unwrap().clone())
    }
    async fn get_spec(&self, spec_id: &str, _jwt: Option<&str>) -> DomainResult<SpecDescriptor> {
        self.single_spec
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| DomainError::Status {
                status: 404,
                reason: Some("Not Found"),
                body: format!("no spec configured for id {spec_id}"),
            })
    }
    async fn create_spec(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: u32,
        _: Option<&str>,
    ) -> DomainResult<SpecDescriptor> {
        unused("create_spec")
    }
    async fn update_spec(
        &self,
        _: &str,
        _: Option<&str>,
        _: Option<&str>,
        _: Option<&str>,
        _: Option<&str>,
    ) -> DomainResult<SpecDescriptor> {
        unused("update_spec")
    }
    async fn delete_spec(&self, _: &str, _: Option<&str>) -> DomainResult<()> {
        unused("delete_spec")
    }
    async fn list_tasks(
        &self,
        _: &str,
        _: Option<&str>,
        _: Option<&str>,
    ) -> DomainResult<Vec<TaskDescriptor>> {
        Ok(vec![])
    }
    async fn create_task(
        &self,
        _task: NewTask<'_>,
        _: Option<&str>,
    ) -> DomainResult<TaskDescriptor> {
        unused("create_task")
    }
    async fn update_task(
        &self,
        _: &str,
        _: TaskUpdate,
        _: Option<&str>,
    ) -> DomainResult<TaskDescriptor> {
        unused("update_task")
    }
    async fn delete_task(&self, _: &str, _: Option<&str>) -> DomainResult<()> {
        unused("delete_task")
    }
    async fn transition_task(
        &self,
        _: &str,
        _: &str,
        _: Option<&str>,
    ) -> DomainResult<TaskDescriptor> {
        unused("transition_task")
    }
    async fn claim_next_task(
        &self,
        _: &str,
        _: &str,
        _: Option<&str>,
    ) -> DomainResult<Option<TaskDescriptor>> {
        Ok(None)
    }
    async fn get_task(&self, _: &str, _: Option<&str>) -> DomainResult<TaskDescriptor> {
        unused("get_task")
    }
    async fn get_project(&self, _: &str, _: Option<&str>) -> DomainResult<ProjectDescriptor> {
        unused("get_project")
    }
    async fn update_project(
        &self,
        _: &str,
        _: ProjectUpdate,
        _: Option<&str>,
    ) -> DomainResult<ProjectDescriptor> {
        unused("update_project")
    }
    async fn create_log(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: Option<&str>,
        _: Option<&serde_json::Value>,
        _: Option<&str>,
    ) -> DomainResult<serde_json::Value> {
        Ok(serde_json::json!({}))
    }
    async fn list_logs(
        &self,
        _: &str,
        _: Option<&str>,
        _: Option<u64>,
        _: Option<&str>,
    ) -> DomainResult<serde_json::Value> {
        Ok(serde_json::json!([]))
    }
    async fn get_project_stats(&self, _: &str, _: Option<&str>) -> DomainResult<serde_json::Value> {
        Ok(serde_json::json!({}))
    }
    async fn list_messages(&self, _: &str, _: &str) -> DomainResult<Vec<MessageDescriptor>> {
        Ok(vec![])
    }
    async fn save_message(&self, _: SaveMessageParams) -> DomainResult<()> {
        Ok(())
    }
    async fn create_session(&self, _: CreateSessionParams) -> DomainResult<SessionDescriptor> {
        unused("create_session")
    }
    async fn get_active_session(&self, _: &str) -> DomainResult<Option<SessionDescriptor>> {
        Ok(None)
    }
    async fn network_api_call(
        &self,
        _: &str,
        _: &str,
        _: Option<&serde_json::Value>,
        _: Option<&str>,
    ) -> DomainResult<String> {
        Ok("{}".into())
    }
}

fn make_spec(id: &str, content: String) -> SpecDescriptor {
    SpecDescriptor {
        id: id.into(),
        project_id: "p1".into(),
        title: format!("spec {id}"),
        content,
        order: 0,
        parent_id: None,
        content_hash: None,
    }
}

fn parse_envelope(raw: &str) -> Value {
    serde_json::from_str::<Value>(raw).expect("tool result must be JSON")
}

#[tokio::test]
async fn test_get_spec_truncates_large_markdown() {
    let total_bytes = 200 * 1024;
    let big = "a".repeat(total_bytes);
    let api = SpecMockApi::with_single_spec(make_spec("s1", big));
    let input = json!({ "spec_id": "s1" });

    let raw = get_spec(&api, "p1", &input).await;
    let env = parse_envelope(&raw);
    assert_eq!(env["ok"], json!(true), "envelope must report ok");
    let spec = &env["spec"];

    let body = spec["markdown_contents"]
        .as_str()
        .expect("markdown_contents must be present as a string");
    let marker = spec_truncation_marker(total_bytes - MAX_SPEC_MARKDOWN_BYTES, total_bytes);
    let expected_len = MAX_SPEC_MARKDOWN_BYTES + marker.len();
    assert_eq!(
        body.len(),
        expected_len,
        "body must be exactly MAX_SPEC_MARKDOWN_BYTES plus the truncation marker"
    );
    assert!(body.len() <= MAX_SPEC_MARKDOWN_BYTES + marker.len());
    assert!(
        body.contains("[truncated"),
        "body must carry the truncation marker substring"
    );
    assert_eq!(
        spec["truncated_markdown"],
        json!(true),
        "structured truncation flag must be set"
    );
    assert_eq!(
        spec["total_markdown_bytes"],
        json!(total_bytes),
        "total_markdown_bytes must report the original (pre-truncation) size"
    );
    assert_eq!(
        spec["id"],
        json!("s1"),
        "non-markdown fields must pass through unchanged"
    );
    assert_eq!(spec["title"], json!("spec s1"));
}

#[tokio::test]
async fn test_get_spec_passes_small_specs_through_unchanged() {
    let body = "x".repeat(1024);
    let api = SpecMockApi::with_single_spec(make_spec("s2", body.clone()));
    let input = json!({ "spec_id": "s2" });

    let raw = get_spec(&api, "p1", &input).await;
    let env = parse_envelope(&raw);
    let spec = &env["spec"];

    assert_eq!(
        spec["markdown_contents"].as_str(),
        Some(body.as_str()),
        "small bodies must pass through byte-for-byte"
    );
    assert!(
        spec.get("truncated_markdown").is_none() || spec["truncated_markdown"] == json!(false),
        "truncated_markdown must be absent (or false) when the body fits"
    );
    assert!(
        spec.get("total_markdown_bytes").is_none(),
        "total_markdown_bytes is only added when the body was truncated"
    );
}

#[tokio::test]
async fn test_list_specs_strips_markdown_contents() {
    let body_a = "a".repeat(4096);
    let body_b = "b".repeat(12345);
    let api = SpecMockApi::with_listing(vec![
        make_spec("s1", body_a.clone()),
        make_spec("s2", body_b.clone()),
    ]);

    let raw = list_specs(&api, "p1", &json!({})).await;
    let env = parse_envelope(&raw);
    let listing = env["specs"]
        .as_array()
        .expect("specs envelope must contain an array");
    assert_eq!(listing.len(), 2, "every input spec must be represented");

    let entry_a = &listing[0];
    assert!(
        entry_a.get("markdown_contents").is_none(),
        "list_specs entries must never ship markdown_contents"
    );
    assert!(
        entry_a.get("content").is_none(),
        "list_specs entries must not leak the raw content field either"
    );
    assert_eq!(
        entry_a["spec_id"],
        json!("s1"),
        "id metadata must round-trip"
    );
    assert_eq!(entry_a["title"], json!("spec s1"));
    assert_eq!(
        entry_a["markdown_bytes"],
        json!(body_a.len()),
        "markdown_bytes must mirror the input length"
    );

    let entry_b = &listing[1];
    assert!(entry_b.get("markdown_contents").is_none());
    assert_eq!(entry_b["spec_id"], json!("s2"));
    assert_eq!(entry_b["markdown_bytes"], json!(body_b.len()));
}
