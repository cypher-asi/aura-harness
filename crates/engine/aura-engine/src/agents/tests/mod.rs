use std::sync::Arc;

use std::time::Duration;

use async_trait::async_trait;

use aura_agent_kernel::{ChildAgentSpec, SpawnHook};

use aura_core_types::{
    AgentId, AgentPermissions, Capability, SpawnMode, SubagentDispatchRequest, SubagentExit,
    ToolResultContent, UserToolDefaults,
};

use aura_exec_tools::catalog::ToolProfile;

use aura_exec_tools::{
    AgentControlHook, AgentMessage, AgentReadHook, SubagentDispatchHook, ToolCatalog, ToolConfig,
};

use aura_model::{
    ContentBlock, Message, ModelProvider, ModelRequest, ModelResponse, ProviderTrace,
    ReasonerError, Role, StopReason, Usage,
};

use aura_store_db::{RocksStore, Store};

use tokio::sync::mpsc;

use super::*;

use crate::scheduler::Scheduler;

/// Answers from the last message so that many agents can share one
/// provider:
///
/// - a tool result → `finished: <result>`
/// - `level N` with N > 1 → a `task` call with `level N-1`
/// - `slow` → waits, then answers
/// - `what number` → `history=<message count>`
/// - anything else → `ok` (`leaf` for `level 1`)
struct ScriptedProvider;

fn last_text(message: &Message) -> String {
    message
        .content
        .iter()
        .find_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn tool_result(message: &Message) -> Option<String> {
    message.content.iter().find_map(|block| match block {
        ContentBlock::ToolResult { content, .. } => Some(match content {
            ToolResultContent::Text(text) => text.clone(),
            ToolResultContent::Json(value) => value.to_string(),
        }),
        _ => None,
    })
}

fn reply(content: Vec<ContentBlock>, stop_reason: StopReason) -> ModelResponse {
    ModelResponse {
        stop_reason,
        message: Message {
            role: Role::Assistant,
            content,
        },
        usage: Usage::new(10, 5),
        trace: ProviderTrace::new("scripted", 0),
        model_used: "scripted".to_string(),
    }
}

fn text(text: impl Into<String>) -> ModelResponse {
    reply(vec![ContentBlock::text(text)], StopReason::EndTurn)
}

#[async_trait]
impl ModelProvider for ScriptedProvider {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        let last = request
            .messages
            .last()
            .cloned()
            .unwrap_or_else(|| Message::user(""));
        if let Some(result) = tool_result(&last) {
            return Ok(text(format!("finished: {result}")));
        }
        let prompt = last_text(&last);
        if let Some(level) = prompt
            .strip_prefix("level ")
            .and_then(|n| n.trim().parse::<u32>().ok())
        {
            if level <= 1 {
                return Ok(text("leaf"));
            }
            return Ok(reply(
                vec![ContentBlock::tool_use(
                    format!("call-{level}"),
                    "task",
                    serde_json::json!({
                        "subagent_type": "general_purpose",
                        "prompt": format!("level {}", level - 1),
                    }),
                )],
                StopReason::ToolUse,
            ));
        }
        if prompt.contains("slow") {
            tokio::time::sleep(Duration::from_secs(30)).await;
            return Ok(text("slow done"));
        }
        if prompt.contains("what number") {
            return Ok(text(format!("history={}", request.messages.len())));
        }
        Ok(text("ok"))
    }

    async fn health_check(&self) -> bool {
        true
    }
}

struct CatalogFactory {
    catalog: Arc<ToolCatalog>,
}

impl ChildKernelFactory for CatalogFactory {
    fn build_child(&self, request: &ChildKernelRequest) -> ChildKernel {
        let config = ToolConfig::default();
        ChildKernel {
            resolver: aura_exec_tools::ToolResolver::new(self.catalog.clone(), config.clone()),
            tools: self.catalog.visible_tools_with_permissions(
                ToolProfile::Agent,
                &config,
                Some(&request.permissions),
            ),
        }
    }
}

struct CollectInbox(mpsc::UnboundedSender<InboxMessage>);

#[async_trait]
impl ParentInbox for CollectInbox {
    async fn deliver(&self, message: InboxMessage) -> Result<(), String> {
        self.0.send(message).map_err(|e| e.to_string())
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    host: AgentHost,
    root: AgentId,
    hooks: AgentHooks,
    inbox: mpsc::UnboundedReceiver<InboxMessage>,
}

fn harness(config: AgentHostConfig) -> Harness {
    harness_observed(config, None)
}

fn harness_observed(
    config: AgentHostConfig,
    observer: Option<Arc<dyn aura_agent_kernel::RecordObserver>>,
) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(dir.path().join("db"), false).unwrap());
    let workspace = dir.path().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let mut scheduler = Scheduler::new(
        store.clone(),
        Arc::new(ScriptedProvider),
        Vec::new(),
        Vec::new(),
        workspace.clone(),
        None,
    );
    if let Some(observer) = observer {
        scheduler = scheduler.with_record_observer(observer);
    }
    let scheduler = Arc::new(scheduler);
    let host = AgentHost::new(store, scheduler, config);
    let root = AgentId::generate();
    let (tx, inbox) = mpsc::unbounded_channel();
    let mut env = AgentEnv::new(Arc::new(CatalogFactory {
        catalog: Arc::new(ToolCatalog::new()),
    }));
    env.workspace = Some((workspace, true));
    host.register_root(RootAgent {
        agent_id: root,
        name: "root".into(),
        permissions: AgentPermissions::full_access(),
        tool_permissions: None,
        mode: aura_core_types::AgentMode::Agent,
        model: "mock".into(),
        env,
        inbox: Some(Arc::new(CollectInbox(tx))),
    });
    let hooks = host.hooks_for(root);
    Harness {
        _dir: dir,
        host,
        root,
        hooks,
        inbox,
    }
}

fn task_request(root: AgentId, prompt: &str) -> SubagentDispatchRequest {
    SubagentDispatchRequest {
        parent_agent_id: root,
        subagent_type: "general_purpose".into(),
        prompt: prompt.into(),
        originating_user_id: None,
        parent_chain: vec![root],
        model_override: None,
        reasoning_effort_override: None,
        system_prompt_addendum: None,
        parent_permissions: AgentPermissions::full_access(),
        parent_tool_permissions: None,
        user_tool_defaults: UserToolDefaults::full_access(),
        tool_call_id: None,
        parent_mode: None,
        parent_kernel_mode: None,
        parent_model_id: Some("mock".into()),
        override_mode: None,
        override_permissions: None,
        override_tool_subset: None,
        override_isolation_id: None,
        override_budget: None,
        spawn_mode: None,
        council_index: None,
        council_parent_tool_use_id: None,
        check: None,
        todo_id: None,
    }
}

fn helper_spec(name: &str, prompt: Option<&str>) -> ChildAgentSpec {
    ChildAgentSpec {
        name: name.into(),
        role: "helper".into(),
        permissions: AgentPermissions::full_access(),
        tool_permissions: None,
        parent_tool_permissions: None,
        system_prompt_override: None,
        preassigned_agent_id: None,
        initial_prompt: prompt.map(ToString::to_string),
    }
}

async fn next_message(inbox: &mut mpsc::UnboundedReceiver<InboxMessage>) -> InboxMessage {
    tokio::time::timeout(Duration::from_secs(20), inbox.recv())
        .await
        .expect("timed out waiting for a message")
        .expect("inbox closed")
}

#[derive(Debug, Default)]
struct SeenEntries(parking_lot::Mutex<Vec<aura_core_types::RecordEntry>>);

impl aura_agent_kernel::RecordObserver for SeenEntries {
    fn on_entry(&self, entry: &aura_core_types::RecordEntry) {
        self.0.lock().push(entry.clone());
    }
}

#[derive(Default)]
struct SeenTurns(parking_lot::Mutex<Vec<String>>);

impl TurnObserver for SeenTurns {
    fn turn_started(&self, turn: &HostedTurn<'_>) {
        self.0.lock().push(format!(
            "start {} parent={}",
            turn.prompt,
            turn.parent.is_some()
        ));
    }

    fn on_event(&self, _agent_id: AgentId, _event: &aura_agent::AgentLoopEvent) {
        let mut seen = self.0.lock();
        if seen.last().map(String::as_str) != Some("event") {
            seen.push("event".into());
        }
    }

    fn turn_ended(
        &self,
        _agent_id: AgentId,
        result: Option<&aura_agent::AgentLoopResult>,
        exit: &SubagentExit,
    ) {
        self.0.lock().push(format!(
            "end result={} {exit:?}",
            result.is_some_and(|r| r.total_text.contains("leaf"))
        ));
    }

    fn agent_removed(&self, _agent_id: AgentId) {
        self.0.lock().push("removed".into());
    }
}

struct CollectMail(mpsc::UnboundedSender<MailMessage>);

impl MailObserver for CollectMail {
    fn on_mail(&self, message: &MailMessage) {
        // The receiver outlives every host in these tests; a closed
        // channel only means the test has already failed.
        let _ = self.0.send(message.clone());
    }
}

fn observe_mail(host: &AgentHost) -> mpsc::UnboundedReceiver<MailMessage> {
    let (tx, rx) = mpsc::unbounded_channel();
    host.set_mail_observer(Arc::new(CollectMail(tx)));
    rx
}

async fn next_mail(mail: &mut mpsc::UnboundedReceiver<MailMessage>) -> MailMessage {
    tokio::time::timeout(Duration::from_secs(10), mail.recv())
        .await
        .expect("timed out waiting for mail")
        .expect("mail channel closed")
}

fn assert_no_mail(mail: &mut mpsc::UnboundedReceiver<MailMessage>) {
    match mail.try_recv() {
        Err(mpsc::error::TryRecvError::Empty) => {}
        other => panic!("expected no mail, got {other:?}"),
    }
}

/// A router that knows one remote agent and records what it is asked
/// to carry.
struct RecordingRouter {
    remote: AgentId,
    sent: parking_lot::Mutex<Vec<RemoteMail>>,
}

impl RemoteRouter for RecordingRouter {
    fn name_of(&self, agent: AgentId) -> Option<String> {
        (agent == self.remote).then(|| "cloud-scout".to_string())
    }

    fn route(&self, mail: RemoteMail) -> Result<(), String> {
        self.sent.lock().push(mail);
        Ok(())
    }

    fn peers(&self, _root: AgentId) -> Vec<serde_json::Value> {
        vec![
            serde_json::json!({"agent_id": self.remote.to_hex(), "name": "cloud-scout", "node": "aura-x"}),
        ]
    }
}

mod lifecycle;
mod mail;
mod remote_mail;
mod spawning;
