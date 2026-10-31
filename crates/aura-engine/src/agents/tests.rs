use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use aura_agent_kernel::{ChildAgentSpec, SpawnHook};
use aura_core_types::{
    AgentId, AgentPermissions, Capability, SpawnMode, SubagentDispatchRequest, SubagentExit,
    ToolResultContent, UserToolDefaults,
};
use aura_model_reasoner::{
    ContentBlock, Message, ModelProvider, ModelRequest, ModelResponse, ProviderTrace,
    ReasonerError, Role, StopReason, Usage,
};
use aura_store_db::{RocksStore, Store};
use aura_tools::catalog::ToolProfile;
use aura_tools::{AgentControlHook, AgentReadHook, SubagentDispatchHook, ToolCatalog, ToolConfig};
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
            resolver: aura_tools::ToolResolver::new(self.catalog.clone(), config.clone()),
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
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(dir.path().join("db"), false).unwrap());
    let workspace = dir.path().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let scheduler = Arc::new(Scheduler::new(
        store.clone(),
        Arc::new(ScriptedProvider),
        Vec::new(),
        Vec::new(),
        workspace.clone(),
        None,
    ));
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

#[tokio::test]
async fn task_children_spawn_their_own_children() {
    let h = harness(AgentHostConfig::default());

    let result = h
        .hooks
        .dispatch(task_request(h.root, "level 3"))
        .await
        .unwrap();

    assert_eq!(result.exit, SubagentExit::Completed, "{result:?}");
    assert!(result.final_message.contains("leaf"), "{result:?}");
    assert!(
        result.final_message.matches("finished").count() >= 2,
        "level 3 and level 2 should both have called task: {result:?}"
    );
    assert_eq!(
        h.host.tree(h.root).len(),
        1,
        "finished task children are removed"
    );
}

#[tokio::test]
async fn task_child_is_visible_while_it_runs() {
    let h = harness(AgentHostConfig::default());
    let hooks = h.hooks.clone();
    let root = h.root;
    let run = tokio::spawn(async move { hooks.dispatch(task_request(root, "slow")).await });

    let mut seen = None;
    for _ in 0..200 {
        if let Some(child) = h.host.tree(h.root).into_iter().nth(1) {
            seen = Some(child);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let child = seen.expect("task child should be listed");
    assert_eq!(child.depth, 1);
    assert_eq!(child.parent, Some(h.root));

    h.hooks
        .lifecycle(&child.agent_id.to_hex(), None, None, "stop")
        .await
        .unwrap();
    let result = run.await.unwrap().unwrap();
    assert_eq!(result.exit, SubagentExit::Cancelled);
}

#[tokio::test]
async fn spawning_past_the_depth_cap_is_rejected() {
    let h = harness(AgentHostConfig {
        max_depth: 3,
        ..AgentHostConfig::default()
    });
    let mut request = task_request(h.root, "level 1");
    request.parent_chain = vec![h.root, AgentId::generate(), AgentId::generate()];

    let result = h.hooks.dispatch(request).await.unwrap();

    assert!(
        matches!(&result.exit, SubagentExit::Rejected { reason } if reason.contains("depth")),
        "{result:?}"
    );
    assert_eq!(h.host.tree(h.root).len(), 1);
}

#[tokio::test]
async fn children_cannot_widen_permissions() {
    let h = harness(AgentHostConfig::default());
    let mut request = task_request(h.root, "level 1");
    request.parent_permissions = AgentPermissions {
        capabilities: vec![Capability::SpawnAgent],
        ..AgentPermissions::empty()
    };
    request.override_permissions = Some(AgentPermissions {
        capabilities: vec![Capability::SpawnAgent, Capability::InvokeProcess],
        ..AgentPermissions::empty()
    });

    let result = h.hooks.dispatch(request).await.unwrap();

    assert!(
        matches!(result.exit, SubagentExit::Rejected { .. }),
        "{result:?}"
    );
}

#[tokio::test]
async fn detached_task_result_reaches_the_parent() {
    let mut h = harness(AgentHostConfig::default());
    let mut request = task_request(h.root, "level 1");
    request.spawn_mode = Some(SpawnMode::Detached);

    let ack = h.hooks.dispatch(request).await.unwrap();
    let child = ack.child_agent_id.expect("child id");

    let message = next_message(&mut h.inbox).await;
    assert_eq!(message.to, h.root);
    assert_eq!(message.from, child);
    assert_eq!(message.kind, MessageKind::Reply);
    assert_eq!(message.text, "leaf");
}

#[tokio::test]
async fn spawned_agent_keeps_its_conversation() {
    let mut h = harness(AgentHostConfig::default());

    let outcome = h
        .hooks
        .spawn_child(&h.root, None, helper_spec("helper", Some("remember 42")))
        .await
        .unwrap();
    let first = next_message(&mut h.inbox).await;
    assert_eq!(first.from, outcome.child_agent_id);
    assert_eq!(first.text, "ok");

    h.hooks
        .deliver_message("helper", None, None, None, "what number?", None, None)
        .await
        .unwrap();
    let second = next_message(&mut h.inbox).await;
    let count: usize = second
        .text
        .strip_prefix("history=")
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("unexpected reply {:?}", second.text));
    assert!(count >= 3, "second turn should see the first: {count}");

    let info = h.host.info(outcome.child_agent_id).unwrap();
    assert_eq!(info.state, AgentState::Idle);
    assert_eq!(info.turns, 2);
}

#[tokio::test]
async fn stopping_an_agent_cancels_it_and_rejects_messages() {
    let h = harness(AgentHostConfig::default());
    let outcome = h
        .hooks
        .spawn_child(&h.root, None, helper_spec("sleeper", Some("slow")))
        .await
        .unwrap();
    let child = outcome.child_agent_id;
    for _ in 0..200 {
        if h.host.info(child).unwrap().state == AgentState::Running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    h.hooks
        .lifecycle(&child.to_hex(), None, None, "stop")
        .await
        .unwrap();

    assert!(h.host.info(child).is_none());
    let err = h
        .hooks
        .deliver_message("sleeper", None, None, None, "hello", None, None)
        .await
        .unwrap_err();
    assert!(err.contains("unknown agent"), "{err}");
}

#[tokio::test]
async fn list_and_snapshot_see_the_callers_tree() {
    let h = harness(AgentHostConfig::default());
    let outcome = h
        .hooks
        .spawn_child(&h.root, None, helper_spec("helper", None))
        .await
        .unwrap();

    let listed = h.hooks.list_agents(None).await.unwrap();
    let names: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["name"].as_str())
        .collect();
    assert_eq!(names, vec!["root", "helper"]);

    let prefix = &outcome.child_agent_id.to_hex()[..12];
    let snapshot = h.hooks.snapshot(prefix).await.unwrap();
    assert_eq!(snapshot["name"], "helper");
    assert_eq!(snapshot["state"], "idle");

    let stranger = h.host.hooks_for(AgentId::generate());
    assert!(stranger.snapshot("helper").await.is_err());
}
