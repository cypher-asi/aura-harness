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
    AgentControlHook, AgentReadHook, SubagentDispatchHook, ToolCatalog, ToolConfig,
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

#[derive(Debug, Default)]
struct SeenEntries(parking_lot::Mutex<Vec<aura_core_types::RecordEntry>>);

impl aura_agent_kernel::RecordObserver for SeenEntries {
    fn on_entry(&self, entry: &aura_core_types::RecordEntry) {
        self.0.lock().push(entry.clone());
    }
}

#[tokio::test]
async fn observer_sees_the_spawn_record_and_the_childs_entries() {
    let seen = Arc::new(SeenEntries::default());
    let h = harness_observed(AgentHostConfig::default(), Some(seen.clone()));

    let result = h
        .hooks
        .dispatch(task_request(h.root, "level 1"))
        .await
        .unwrap();
    assert_eq!(result.exit, SubagentExit::Completed, "{result:?}");

    let seen = seen.0.lock();
    let spawn = seen
        .iter()
        .find(|e| e.tx.tx_type == aura_core_types::TransactionType::SubagentSpawn)
        .expect("spawn record should be observed");
    assert_eq!(spawn.tx.agent_id, h.root);
    let child = result.child_agent_id.unwrap();
    assert!(
        seen.iter().any(|e| e.tx.agent_id == child
            && e.tx.tx_type == aura_core_types::TransactionType::Reasoning),
        "the child's turn should be observed"
    );
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

#[tokio::test]
async fn turn_observer_hears_each_turn_in_order() {
    let h = harness(AgentHostConfig::default());
    let seen = Arc::new(SeenTurns::default());
    h.host.set_turn_observer(seen.clone());

    let result = h
        .hooks
        .dispatch(task_request(h.root, "level 1"))
        .await
        .unwrap();
    assert_eq!(result.exit, SubagentExit::Completed, "{result:?}");

    let seen = seen.0.lock().clone();
    assert_eq!(
        seen,
        vec![
            "start level 1 parent=true".to_string(),
            "event".to_string(),
            "end result=true Completed".to_string(),
            "removed".to_string(),
        ]
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

#[tokio::test]
async fn spawn_agent_mail_request_and_reply_share_a_thread() {
    let mut h = harness(AgentHostConfig::default());
    let mut mail = observe_mail(&h.host);

    let outcome = h
        .hooks
        .spawn_child(
            &h.root,
            None,
            helper_spec("helper", Some("review the parser")),
        )
        .await
        .unwrap();
    let child = outcome.child_agent_id;

    let request = next_mail(&mut mail).await;
    assert_eq!(request.kind, MessageKind::Request, "{request:?}");
    assert_eq!(request.from, h.root);
    assert_eq!(request.from_name, "root");
    assert_eq!(request.to, child);
    assert_eq!(request.to_name, "helper");
    assert_eq!(request.text, "review the parser");
    assert_eq!(request.subject, "review the parser");

    let reply = next_mail(&mut mail).await;
    assert_eq!(reply.kind, MessageKind::Reply, "{reply:?}");
    assert_eq!(reply.from, child);
    assert_eq!(reply.from_name, "helper");
    assert_eq!(reply.to, h.root);
    assert_eq!(reply.to_name, "root");
    assert_eq!(reply.text, "ok");
    assert_eq!(reply.thread, request.thread);
    assert_eq!(reply.subject, request.subject);
    assert!(reply.id > request.id, "ids increase: {request:?} {reply:?}");

    let delivered = next_message(&mut h.inbox).await;
    assert_eq!(delivered.from, child);
    assert_eq!(delivered.to, h.root);
    assert_eq!(delivered.kind, MessageKind::Reply);
    assert_eq!(delivered.thread, request.thread);
    assert_eq!(delivered.subject, request.subject);
    assert_no_mail(&mut mail);
}

#[tokio::test]
async fn mail_subject_threads_messages_sent_through_the_hook() {
    let mut h = harness(AgentHostConfig::default());
    let mut mail = observe_mail(&h.host);
    let child = h
        .hooks
        .spawn_child(&h.root, None, helper_spec("helper", None))
        .await
        .unwrap()
        .child_agent_id;
    assert_no_mail(&mut mail);
    let child_hooks = h.host.hooks_for(child);
    let root_hex = h.root.to_hex();

    let sends = [
        ("first finding", "Parser review"),
        ("second finding", "Parser review"),
        ("something else", "Other topic"),
    ];
    for (text, subject) in sends {
        child_hooks
            .deliver_message_in_thread(&root_hex, None, None, None, text, None, None, Some(subject))
            .await
            .unwrap();
    }

    let mut posted = Vec::new();
    for _ in 0..sends.len() {
        posted.push(next_mail(&mut mail).await);
    }
    for (message, (text, subject)) in posted.iter().zip(sends) {
        assert_eq!(message.kind, MessageKind::Request, "{message:?}");
        assert_eq!(message.from, child);
        assert_eq!(message.to, h.root);
        assert_eq!(message.text, text);
        assert_eq!(message.subject, subject);
    }
    assert_eq!(
        posted[0].thread, posted[1].thread,
        "same subject, same thread"
    );
    assert_ne!(
        posted[0].thread, posted[2].thread,
        "new subject, new thread"
    );
    assert_no_mail(&mut mail);

    // Each root delivery runs on its own task, so match them up by text.
    let mut delivered = Vec::new();
    for _ in 0..sends.len() {
        delivered.push(next_message(&mut h.inbox).await);
    }
    for (text, subject) in sends {
        let message = delivered
            .iter()
            .find(|m| m.text == text)
            .unwrap_or_else(|| panic!("{text:?} not delivered: {delivered:?}"));
        let expected = posted.iter().find(|m| m.text == text).unwrap();
        assert_eq!(message.from, child);
        assert_eq!(message.kind, MessageKind::Request);
        assert_eq!(message.subject, subject);
        assert_eq!(message.thread, expected.thread);
        let rendered = message.render();
        assert!(
            rendered.contains(&format!("subject \"{subject}\"")),
            "{rendered}"
        );
    }
}

#[tokio::test]
async fn only_the_mail_that_opens_a_thread_named_from_its_text_is_marked() {
    let h = harness(AgentHostConfig::default());
    let mut mail = observe_mail(&h.host);
    let child = h
        .hooks
        .spawn_child(&h.root, None, helper_spec("helper", None))
        .await
        .unwrap()
        .child_agent_id;
    assert_no_mail(&mut mail);
    let child_hooks = h.host.hooks_for(child);
    let root_hex = h.root.to_hex();

    let sends = [
        ("Found a parser bug\ndetails", None),
        ("and another one", None),
        ("bench numbers", Some("Benchmarks")),
    ];
    for (text, subject) in sends {
        child_hooks
            .deliver_message_in_thread(&root_hex, None, None, None, text, None, None, subject)
            .await
            .unwrap();
    }

    let opened = next_mail(&mut mail).await;
    assert_eq!(opened.subject, "Found a parser bug");
    assert!(opened.subject_from_text, "{opened:?}");

    let continued = next_mail(&mut mail).await;
    assert_eq!(continued.thread, opened.thread);
    assert!(!continued.subject_from_text, "{continued:?}");

    let given = next_mail(&mut mail).await;
    assert_ne!(given.thread, opened.thread);
    assert!(!given.subject_from_text, "{given:?}");
    assert_no_mail(&mut mail);
}

#[tokio::test]
async fn user_messages_are_not_mail() {
    let mut h = harness(AgentHostConfig::default());
    let mut mail = observe_mail(&h.host);
    let child = h
        .hooks
        .spawn_child(&h.root, None, helper_spec("helper", None))
        .await
        .unwrap()
        .child_agent_id;

    // Mail is posted inline in `enqueue`, so its absence is visible as
    // soon as `send` returns.
    h.host
        .send(None, child, "hello from the user".into())
        .unwrap();
    assert_no_mail(&mut mail);
    assert!(h.host.send(None, h.root, "hello root".into()).is_err());
    assert_no_mail(&mut mail);

    // The child handles its queue in order, so once the answer to this
    // agent message is mailed, the user's turn has finished too.
    h.host.send(Some(h.root), child, "ping".into()).unwrap();
    let request = next_mail(&mut mail).await;
    assert_eq!(request.kind, MessageKind::Request, "{request:?}");
    assert_eq!(request.from, h.root);
    assert_eq!(request.text, "ping");
    let reply = next_mail(&mut mail).await;
    assert_eq!(reply.kind, MessageKind::Reply, "{reply:?}");
    assert_eq!(reply.from, child);
    assert_eq!(reply.to, h.root);
    assert_eq!(reply.thread, request.thread);
    let delivered = next_message(&mut h.inbox).await;
    assert_eq!(delivered.thread, request.thread);
    assert_no_mail(&mut mail);
    assert_eq!(h.host.info(child).unwrap().turns, 2);
}

#[tokio::test]
async fn mail_forwarded_from_a_childs_reply_goes_in_the_parents_thread() {
    let mut h = harness(AgentHostConfig::default());
    let mut mail = observe_mail(&h.host);
    let middle = h
        .hooks
        .spawn_child(
            &h.root,
            None,
            helper_spec("middle", Some("review the parser")),
        )
        .await
        .unwrap()
        .child_agent_id;
    let request = next_mail(&mut mail).await;
    assert_eq!((request.from, request.to), (h.root, middle));
    let answer = next_mail(&mut mail).await;
    assert_eq!((answer.from, answer.to), (middle, h.root));
    assert_eq!(answer.thread, request.thread);
    next_message(&mut h.inbox).await;

    // The middle agent starts a child of its own, whose reply wakes it.
    let leaf = h
        .host
        .hooks_for(middle)
        .spawn_child(&middle, None, helper_spec("leaf", Some("check the lexer")))
        .await
        .unwrap()
        .child_agent_id;
    let down = next_mail(&mut mail).await;
    assert_eq!((down.from, down.to), (middle, leaf));
    assert_ne!(
        down.thread, request.thread,
        "a spawn prompt starts a thread"
    );
    let up = next_mail(&mut mail).await;
    assert_eq!(
        (up.from, up.to, up.kind),
        (leaf, middle, MessageKind::Reply)
    );
    assert_eq!(up.thread, down.thread);

    let forwarded = next_mail(&mut mail).await;
    assert_eq!(
        (forwarded.from, forwarded.to, forwarded.kind),
        (middle, h.root, MessageKind::Reply)
    );
    assert_eq!(forwarded.thread, request.thread, "{forwarded:?}");
    assert_eq!(forwarded.subject, request.subject);
    let delivered = next_message(&mut h.inbox).await;
    assert_eq!(delivered.thread, request.thread);
    assert_no_mail(&mut mail);
}

#[tokio::test]
async fn mail_from_a_delegated_task_starts_a_new_thread() {
    let mut h = harness(AgentHostConfig::default());
    let mut mail = observe_mail(&h.host);
    h.hooks
        .spawn_child(
            &h.root,
            None,
            helper_spec("helper", Some("review the parser")),
        )
        .await
        .unwrap();
    let request = next_mail(&mut mail).await;
    let answer = next_mail(&mut mail).await;
    assert_eq!(answer.thread, request.thread);
    next_message(&mut h.inbox).await;

    h.hooks
        .delegate_task("helper", None, None, "Benchmark the lexer", None, None)
        .await
        .unwrap();
    let delegated = next_mail(&mut mail).await;
    assert_eq!(delegated.kind, MessageKind::Request);
    assert_eq!(delegated.text, "Benchmark the lexer");
    assert_ne!(delegated.thread, request.thread, "{delegated:?}");
    assert_eq!(delegated.subject, "Benchmark the lexer");
}

#[tokio::test]
async fn mail_answers_a_message_dropped_by_a_stop_with_a_cancelled_reply() {
    let h = harness(AgentHostConfig::default());
    let mut mail = observe_mail(&h.host);
    let child = h
        .hooks
        .spawn_child(&h.root, None, helper_spec("sleeper", Some("slow")))
        .await
        .unwrap()
        .child_agent_id;
    let first = next_mail(&mut mail).await;
    assert_eq!(first.text, "slow");
    for _ in 0..200 {
        if h.host.info(child).unwrap().state == AgentState::Running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    h.host.send(Some(h.root), child, "ping".into()).unwrap();
    let queued = next_mail(&mut mail).await;
    assert_eq!(queued.text, "ping");
    assert_eq!(h.host.info(child).unwrap().queued, 1);

    h.hooks
        .lifecycle(&child.to_hex(), None, None, "stop")
        .await
        .unwrap();

    let cancelled = next_mail(&mut mail).await;
    assert_eq!(cancelled.kind, MessageKind::Reply, "{cancelled:?}");
    assert_eq!((cancelled.from, cancelled.to), (child, h.root));
    assert_eq!(
        (cancelled.from_name.as_str(), cancelled.to_name.as_str()),
        ("sleeper", "root")
    );
    assert_eq!(cancelled.text, "was cancelled");
    assert_eq!(cancelled.thread, queued.thread);
    assert_eq!(cancelled.subject, queued.subject);
    assert!(cancelled.id > queued.id);
    assert_no_mail(&mut mail);
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

#[tokio::test]
async fn mail_for_an_agent_on_another_node_goes_through_the_router() {
    let h = harness(AgentHostConfig::default());
    let remote = AgentId::generate();
    let router = Arc::new(RecordingRouter {
        remote,
        sent: parking_lot::Mutex::new(Vec::new()),
    });
    h.host.set_remote_router(router.clone());

    h.hooks
        .deliver_message_in_thread(
            &remote.to_hex(),
            None,
            None,
            None,
            "please review the parser",
            None,
            None,
            Some("Parser review"),
        )
        .await
        .expect("routed");
    let sent = router.sent.lock().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].from, h.root);
    assert_eq!(sent[0].to, remote);
    assert_eq!(sent[0].kind, MessageKind::Request);
    assert_eq!(sent[0].subject, "Parser review");

    let listed = h.hooks.list_agents(None).await.expect("list");
    assert!(listed.to_string().contains("cloud-scout"), "{listed}");
}

#[tokio::test]
async fn mail_from_another_node_reaches_a_root_in_its_thread() {
    let mut h = harness(AgentHostConfig::default());
    let remote = AgentId::generate();
    h.host
        .deliver_remote(RemoteMail {
            from: remote,
            from_name: "cloud-scout".into(),
            to: h.root,
            kind: MessageKind::Reply,
            thread: ThreadId("aura-x:t2".into()),
            subject: "Parser review".into(),
            text: "Found two issues".into(),
        })
        .expect("delivered");
    let message = next_message(&mut h.inbox).await;
    assert_eq!(message.from, remote);
    assert_eq!(message.from_name, "cloud-scout");
    assert_eq!(message.kind, MessageKind::Reply);
    assert_eq!(message.thread, ThreadId("aura-x:t2".into()));
}

#[tokio::test]
async fn an_agent_no_node_holds_is_still_unknown() {
    let h = harness(AgentHostConfig::default());
    h.host.set_remote_router(Arc::new(RecordingRouter {
        remote: AgentId::generate(),
        sent: parking_lot::Mutex::new(Vec::new()),
    }));
    let err = h
        .hooks
        .deliver_message(
            &AgentId::generate().to_hex(),
            None,
            None,
            None,
            "hi",
            None,
            None,
        )
        .await
        .expect_err("unknown");
    assert!(err.contains("unknown agent"), "{err}");
}
